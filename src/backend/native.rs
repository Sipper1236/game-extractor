use std::path::Path;
use anyhow::{Result, Context};
use async_trait::async_trait;
use tokio::process::Command;
use crate::backend::ExtractionBackend;
use crate::config::InstallerConfig;
use crate::layout::BinFile;

pub struct NativeBackend {
    pub precomp_path: Option<String>,
    pub zstd_path: Option<String>,
    pub lz4_path: Option<String>,
    pub bzip3_path: Option<String>,
    pub xtool_path: Option<String>,
}

impl NativeBackend {
    pub fn new() -> Self {
        Self {
            precomp_path: which::which("precomp").ok().map(|p| p.to_string_lossy().to_string()),
            zstd_path: which::which("zstd").ok().map(|p| p.to_string_lossy().to_string()),
            lz4_path: which::which("lz4").ok().map(|p| p.to_string_lossy().to_string()),
            bzip3_path: which::which("bzip3").ok().map(|p| p.to_string_lossy().to_string()),
            xtool_path: which::which("xtool").or_else(|_| which::which("xtool-linux")).ok().map(|p| p.to_string_lossy().to_string()),
        }
    }

    fn detect_codec_chain(&self, bin: &BinFile, _config: &InstallerConfig) -> Vec<String> {
            // All CP2077 bins use CLS plugins (srep, lollypop, magic) + other Windows-only codecs
            // Native backend cannot handle any of them
            vec!["srep".to_string(), "lollypop".to_string(), "magic".to_string(), "precomp".to_string(), "zstd".to_string(), "oodle".to_string(), "razor".to_string(), "xtool".to_string()]
        }
}

#[async_trait]
impl ExtractionBackend for NativeBackend {
    fn name(&self) -> &str {
        "native"
    }

    fn can_handle(&self, bin: &BinFile, config: &InstallerConfig) -> bool {
        let codecs = self.detect_codec_chain(bin, config);
        
        // Check if all required codecs are available natively
        for codec in codecs {
            match codec.as_str() {
                "precomp" => if self.precomp_path.is_none() { return false; },
                "zstd" | "pzlib" => if self.zstd_path.is_none() { return false; },
                "lz4" | "plzo" => if self.lz4_path.is_none() { return false; },
                "bzip3" | "bz3" | "bz3s" => if self.bzip3_path.is_none() { return false; },
                "xtool" | "xt" | "xt2" | "xt3" => if self.xtool_path.is_none() { return false; },
                "storing" => {},
                // CLS plugins and Windows-only codecs
                "srep" | "lollypop" | "magic" | "oodle" | "razor" | "msc" => return false,
                _ => return false, // Unknown or unsupported codec
            }
        }
        true
    }

    async fn extract(&self, bin: &BinFile, _config: &InstallerConfig, output_dir: &Path, progress_tx: Option<tokio::sync::mpsc::Sender<u64>>) -> Result<Vec<String>> {
        // This is a placeholder implementation
        // Real implementation would:
        // 1. Parse FreeArc archive structure
        // 2. Extract each block using appropriate codec
        // 3. Reconstruct file tree
        
        tokio::fs::create_dir_all(output_dir).await?;
        
        // For now, just copy the bin file as a placeholder
        let output_file = output_dir.join(&bin.name);
        tokio::fs::copy(&bin.path, &output_file).await?;
        
        // Simulate progress
        if let Some(tx) = progress_tx {
            for i in 0..=100 {
                let _ = tx.send((bin.size * i) / 100).await;
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        }
        
        Ok(vec![output_file.to_string_lossy().to_string()])
    }

    fn get_required_tools(&self) -> Vec<String> {
        let mut tools = Vec::new();
        if self.precomp_path.is_some() { tools.push("precomp".to_string()); }
        if self.zstd_path.is_some() { tools.push("zstd".to_string()); }
        if self.lz4_path.is_some() { tools.push("lz4".to_string()); }
        if self.bzip3_path.is_some() { tools.push("bzip3".to_string()); }
        if self.xtool_path.is_some() { tools.push("xtool/xtool-linux".to_string()); }
        tools
    }
}

impl Default for NativeBackend {
    fn default() -> Self {
        Self::new()
    }
}