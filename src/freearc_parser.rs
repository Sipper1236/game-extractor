//! FreeArc archive parser - reads archive structure and extracts files
//! Based on ArcStructure.h from xredor/unarc

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use anyhow::{Result, Context, bail};
use byteorder::{LittleEndian, ReadBytesExt};
use flate2::read::ZlibDecoder;
use lzma_rs::lzma_decompress;
use zstd::stream::read::Decoder as ZstdDecoder;
use crate::layout::BinFile;

#[derive(Debug, Clone)]
pub struct BlockDescriptor {
    pub block_type: u32,
    pub unpacked_size: u64,
    pub packed_size: u64,
    pub crc32: u32,
    pub flags: u32,
    pub offset: u64, // File offset of this block's data
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub offset: u64, // Offset within the archive
    pub crc32: u32,
    pub method: String,
    pub solid: bool,
}

#[derive(Debug)]
pub struct FreeArcArchive {
    pub blocks: Vec<BlockDescriptor>,
    pub files: Vec<FileEntry>,
    pub main_data_offset: u64,
    pub file: File,
}

impl FreeArcArchive {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        
        let file_size = file.seek(SeekFrom::End(0))?;
        
        // Read the last 500 bytes to find the signature
        let scan_size = 500.min(file_size as usize);
        file.seek(SeekFrom::End(-(scan_size as i64)))?;
        let mut scan_buffer = vec![0u8; scan_size];
        file.read_exact(&mut scan_buffer)?;
        
        // Find "ArC\x01" signature (0x01437241 in LE)
        const SIGNATURE: u32 = 0x01437241;
        let mut sig_pos = None;
        for i in 0..scan_buffer.len().saturating_sub(3) {
            let val = u32::from_le_bytes([
                scan_buffer[i], scan_buffer[i+1], scan_buffer[i+2], scan_buffer[i+3]
            ]);
            if val == SIGNATURE {
                sig_pos = Some(i);
            }
        }
        
        let sig_offset = sig_pos.ok_or_else(|| anyhow::anyhow!("FreeArc signature not found in last {} bytes", scan_size))?;
        let sig_abs_offset = file_size - scan_size as u64 + sig_offset as u64;
        
        // The footer block header is 28 bytes before the descriptor array
        // The signature is at the END of the footer block
        // Footer block structure: header(28) + descriptor_array(packed) + signature(4)
        // So footer header is at: sig_abs_offset - 4 - descriptor_packed_size
        // But we don't know descriptor_packed_size yet...
        // Actually, the footer block header is right before the descriptor array
        // So: [footer_header(28)][descriptor_array(packed)][signature(4)]
        // signature is at sig_abs_offset
        // descriptor_array ends at sig_abs_offset - 4
        // footer_header ends at sig_abs_offset - 4 - descriptor_packed_size
        // footer_header starts at sig_abs_offset - 4 - descriptor_packed_size - 28
        
        // We need to read the footer block header to get the packed_size
        // The footer block header should be at a reasonable distance before the signature
        // Let's scan backwards from signature for a valid block header (type=4)
        
        let mut footer_header_offset = None;
        let max_scan = 1024 * 1024; // Scan up to 1MB back
        let scan_start = sig_abs_offset.saturating_sub(max_scan);
        
        file.seek(SeekFrom::Start(scan_start))?;
        let mut scan_data = vec![0u8; (sig_abs_offset - scan_start) as usize];
        file.read_exact(&mut scan_data)?;
        
        // Search for footer block header (type=4) in the scanned region
        // Block header: type(4), unpacked_size(8), packed_size(8), crc32(4), flags(4) = 28 bytes
        for i in (0..scan_data.len().saturating_sub(28)).step_by(4) {
            let block_type = u32::from_le_bytes([
                scan_data[i], scan_data[i+1], scan_data[i+2], scan_data[i+3]
            ]);
            
            if block_type == 4 {
                // Potential footer block header found
                let unpacked_size = u64::from_le_bytes([
                    scan_data[i+4], scan_data[i+5], scan_data[i+6], scan_data[i+7],
                    scan_data[i+8], scan_data[i+9], scan_data[i+10], scan_data[i+11],
                ]);
                let packed_size = u64::from_le_bytes([
                    scan_data[i+12], scan_data[i+13], scan_data[i+14], scan_data[i+15],
                    scan_data[i+16], scan_data[i+17], scan_data[i+18], scan_data[i+19],
                ]);
                let crc32 = u32::from_le_bytes([
                    scan_data[i+20], scan_data[i+21], scan_data[i+22], scan_data[i+23],
                ]);
                let flags = u32::from_le_bytes([
                    scan_data[i+24], scan_data[i+25], scan_data[i+26], scan_data[i+27],
                ]);
                
                // Check if this header makes sense
                // The descriptor array should end right before the signature
                let header_abs = scan_start + i as u64;
                let descriptor_end = header_abs + 28 + packed_size;
                let expected_sig_pos = descriptor_end;
                
                if expected_sig_pos == sig_abs_offset {
                    footer_header_offset = Some(header_abs);
                    eprintln!("Found footer block header at offset 0x{:x}, packed_size={}, unpacked_size={}, flags=0x{:x}", 
                        header_abs, packed_size, unpacked_size, flags);
                    break;
                }
            }
        }
        
        let footer_header_offset = footer_header_offset
            .ok_or_else(|| anyhow::anyhow!("Footer block header (type=4) not found before signature"))?;
        
        // Read the footer block header
        file.seek(SeekFrom::Start(footer_header_offset))?;
        let block_type = file.read_u32::<LittleEndian>()?;
        let unpacked_size = file.read_u64::<LittleEndian>()?;
        let packed_size = file.read_u64::<LittleEndian>()?;
        let crc32 = file.read_u32::<LittleEndian>()?;
        let flags = file.read_u32::<LittleEndian>()?;
        
        assert_eq!(block_type, 4, "Footer block type should be 4");
        
        // Extract compression method from flags (low 16 bits)
        let method = (flags & 0xFFFF) as u16;
        let solid = (flags & 0x10000) != 0;
        let dict_size = ((flags >> 17) & 0x7FFF) as u32;
        
        eprintln!("Footer block: method={}, solid={}, dict_size={}, packed_size={}, unpacked_size={}", 
            method, solid, dict_size, packed_size, unpacked_size);
        
        // Read the compressed descriptor array
        let descriptor_offset = footer_header_offset + 28;
        file.seek(SeekFrom::Start(descriptor_offset))?;
        let mut descriptor_data = vec![0u8; packed_size as usize];
        file.read_exact(&mut descriptor_data)?;
        
        // Decompress descriptor array based on method
        let decompressed = match method {
            0 => descriptor_data, // Stored
            1 => {
                // LZMA
                let mut input = std::io::Cursor::new(&descriptor_data);
                let mut output = Vec::new();
                lzma_decompress(&mut input, &mut output)
                    .map_err(|e| anyhow::anyhow!("LZMA decompression failed: {}", e))?;
                output
            }
            2 => {
                // PPMD - not implemented, fallback
                bail!("PPMD decompression not implemented");
            }
            3 => {
                // BZip2 - not in FreeArc typically
                bail!("BZip2 decompression not implemented");
            }
            4 => {
                // Zstd (if supported by this FreeArc version)
                let mut decoder = ZstdDecoder::new(std::io::Cursor::new(&descriptor_data))
                    .map_err(|e| anyhow::anyhow!("Zstd decoder creation failed: {}", e))?;
                let mut output = Vec::new();
                decoder.read_to_end(&mut output)
                    .map_err(|e| anyhow::anyhow!("Zstd decompression failed: {}", e))?;
                output
            }
            _ => bail!("Unknown compression method: {}", method),
        };
        
        eprintln!("Decompressed descriptor array: {} bytes", decompressed.len());
        
        // Parse block descriptors from decompressed data
        // Each descriptor: type(4), unpacked_size(8), packed_size(8), crc32(4), flags(4) = 28 bytes
        let num_blocks = decompressed.len() / 28;
        let mut blocks = Vec::new();
        let mut current_offset = 0u64; // Will be updated after we know where data blocks start
        
        for i in 0..num_blocks {
            let base = i * 28;
            let block_type = u32::from_le_bytes([
                decompressed[base], decompressed[base+1], decompressed[base+2], decompressed[base+3]
            ]);
            let unpacked_size = u64::from_le_bytes([
                decompressed[base+4], decompressed[base+5], decompressed[base+6], decompressed[base+7],
                decompressed[base+8], decompressed[base+9], decompressed[base+10], decompressed[base+11],
            ]);
            let packed_size = u64::from_le_bytes([
                decompressed[base+12], decompressed[base+13], decompressed[base+14], decompressed[base+15],
                decompressed[base+16], decompressed[base+17], decompressed[base+18], decompressed[base+19],
            ]);
            let crc32 = u32::from_le_bytes([
                decompressed[base+20], decompressed[base+21], decompressed[base+22], decompressed[base+23],
            ]);
            let flags = u32::from_le_bytes([
                decompressed[base+24], decompressed[base+25], decompressed[base+26], decompressed[base+27],
            ]);
            
            blocks.push(BlockDescriptor {
                block_type,
                unpacked_size,
                packed_size,
                crc32,
                flags,
                offset: 0, // Will be set later
            });
        }
        
        // Now we need to find the actual data blocks in the file
        // The blocks are stored sequentially starting from offset 0
        // But we need to verify by scanning for block headers
        let mut data_offset = 0u64;
        let mut verified_blocks = Vec::new();
        
        // Scan from beginning of file to find all block headers in order
        file.seek(SeekFrom::Start(0))?;
        let mut file_data = Vec::new();
        file.read_to_end(&mut file_data)?;
        
        let mut pos = 0usize;
        let mut block_index = 0;
        
        while pos + 28 <= file_data.len() && block_index < blocks.len() {
            let block_type = u32::from_le_bytes([
                file_data[pos], file_data[pos+1], file_data[pos+2], file_data[pos+3]
            ]);
            
            if block_type == blocks[block_index].block_type {
                let unpacked = u64::from_le_bytes([
                    file_data[pos+4], file_data[pos+5], file_data[pos+6], file_data[pos+7],
                    file_data[pos+8], file_data[pos+9], file_data[pos+10], file_data[pos+11],
                ]);
                let packed = u64::from_le_bytes([
                    file_data[pos+12], file_data[pos+13], file_data[pos+14], file_data[pos+15],
                    file_data[pos+16], file_data[pos+17], file_data[pos+18], file_data[pos+19],
                ]);
                let crc = u32::from_le_bytes([
                    file_data[pos+20], file_data[pos+21], file_data[pos+22], file_data[pos+23],
                ]);
                let flags = u32::from_le_bytes([
                    file_data[pos+24], file_data[pos+25], file_data[pos+26], file_data[pos+27],
                ]);
                
                if unpacked == blocks[block_index].unpacked_size && packed == blocks[block_index].packed_size {
                    blocks[block_index].offset = pos as u64 + 28; // Data starts after header
                    verified_blocks.push(blocks[block_index].clone());
                    pos += 28 + packed as usize;
                    block_index += 1;
                    continue;
                }
            }
            
            pos += 4; // Try next position
        }
        
        if verified_blocks.len() != blocks.len() {
            eprintln!("Warning: Only verified {}/{} blocks", verified_blocks.len(), blocks.len());
        }
        
        // Find directory block (type=3) and parse file table
        let mut files = Vec::new();
        
        for block in &verified_blocks {
            if block.block_type == 3 {
                // Directory block - parse file entries
                file.seek(SeekFrom::Start(block.offset))?;
                let mut dir_data = vec![0u8; block.packed_size as usize];
                file.read_exact(&mut dir_data)?;
                
                // Decompress if needed
                let method = (block.flags & 0xFFFF) as u16;
                let decompressed = match method {
                    0 => dir_data,
                    1 => {
                        let mut input = std::io::Cursor::new(&dir_data);
                        let mut output = Vec::new();
                        lzma_decompress(&mut input, &mut output)
                            .map_err(|e| anyhow::anyhow!("LZMA decompression failed: {}", e))?;
                        output
                    }
                    4 => {
                        let mut decoder = ZstdDecoder::new(std::io::Cursor::new(&dir_data))
                            .map_err(|e| anyhow::anyhow!("Zstd decoder creation failed: {}", e))?;
                        let mut output = Vec::new();
                        decoder.read_to_end(&mut output)
                            .map_err(|e| anyhow::anyhow!("Zstd decompression failed: {}", e))?;
                        output
                    }
                    _ => bail!("Unsupported directory block compression: {}", method),
                };
                
                // Parse file entries from directory block
                // Format: count(4), then for each file: name_len(2), name, size(8), offset(8), crc32(4), method_str_len(2), method_str
                let mut cursor = std::io::Cursor::new(&decompressed);
                let file_count = cursor.read_u32::<LittleEndian>()?;
                
                for _ in 0..file_count {
                    let name_len = cursor.read_u16::<LittleEndian>()? as usize;
                    let mut name_buf = vec![0u8; name_len];
                    cursor.read_exact(&mut name_buf)?;
                    let name = String::from_utf8_lossy(&name_buf).to_string();
                    
                    let size = cursor.read_u64::<LittleEndian>()?;
                    let offset = cursor.read_u64::<LittleEndian>()?;
                    let crc32 = cursor.read_u32::<LittleEndian>()?;
                    let method_len = cursor.read_u16::<LittleEndian>()? as usize;
                    let mut method_buf = vec![0u8; method_len];
                    cursor.read_exact(&mut method_buf)?;
                    let method = String::from_utf8_lossy(&method_buf).to_string();
                    
                    files.push(FileEntry {
                        name,
                        size,
                        offset,
                        crc32,
                        method,
                        solid: (block.flags & 0x10000) != 0,
                    });
                }
                
                break;
            }
        }
        
        // Re-open file for extraction
        let file = File::open(path)?;
        
        Ok(Self {
            blocks: verified_blocks,
            files,
            main_data_offset: 0, // Data blocks start at beginning of file
            file,
        })
    }
    
    pub fn extract_all(&mut self, output_dir: &Path) -> Result<Vec<PathBuf>> {
        std::fs::create_dir_all(output_dir)?;
        
        let mut extracted = Vec::new();
        
        for file_entry in &self.files {
            let output_path = output_dir.join(&file_entry.name);
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            
            // Find the data block containing this file
            // For simplicity, assume each file is in its own block (type=1 or 2)
            // In reality, solid archives pack multiple files per block
            
            // Find data block(s) for this file
            for block in &self.blocks {
                if block.block_type == 1 || block.block_type == 2 {
                    // Data block
                    self.file.seek(SeekFrom::Start(block.offset))?;
                    let mut block_data = vec![0u8; block.packed_size as usize];
                    self.file.read_exact(&mut block_data)?;
                    
                    // Decompress
                    let method = (block.flags & 0xFFFF) as u16;
                    let decompressed = match method {
                        0 => block_data,
                        1 => {
                            let mut input = std::io::Cursor::new(&block_data);
                            let mut output = Vec::new();
                            lzma_decompress(&mut input, &mut output)
                                .map_err(|e| anyhow::anyhow!("LZMA decompression failed: {}", e))?;
                            output
                        }
                        4 => {
                            let mut decoder = ZstdDecoder::new(std::io::Cursor::new(&block_data))
                                .map_err(|e| anyhow::anyhow!("Zstd decoder creation failed: {}", e))?;
                            let mut output = Vec::new();
                            decoder.read_to_end(&mut output)
                                .map_err(|e| anyhow::anyhow!("Zstd decompression failed: {}", e))?;
                            output
                        }
                        _ => {
                            eprintln!("Unsupported block compression method: {}", method);
                            continue;
                        }
                    };
                    
                    // Extract the specific file from this block
                    // In solid mode, multiple files are concatenated
                    // For now, just write the whole decompressed block if it matches file size
                    if decompressed.len() as u64 == file_entry.size {
                        std::fs::write(&output_path, &decompressed)?;
                        extracted.push(output_path.clone());
                        break;
                    }
                }
            }
        }
        
        Ok(extracted)
    }
}

pub fn parse_freearc_archive(path: &Path) -> Result<Vec<FileEntry>> {
    let archive = FreeArcArchive::open(path)?;
    Ok(archive.files)
}