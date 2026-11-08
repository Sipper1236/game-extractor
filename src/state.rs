use std::collections::HashMap;
use std::path::{Path, PathBuf};
use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use hex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionState {
    pub installer_hash: String,
    pub output_dir: PathBuf,
    pub completed_bins: HashMap<String, BinState>,
    pub current_bin: Option<String>,
    pub current_offset: u64,
    pub total_bins: usize,
    pub total_bytes: u64,
    pub extracted_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinState {
    pub name: String,
    pub size: u64,
    pub extracted_size: u64,
    pub completed: bool,
    pub checksum: Option<String>,
    pub output_files: Vec<String>,
}

impl ExtractionState {
    pub fn new(installer_dir: &Path, output_dir: &Path, bin_files: &[crate::layout::BinFile]) -> Result<Self> {
        let installer_hash = compute_installer_hash(installer_dir)?;
        let total_bins = bin_files.len();
        let total_bytes: u64 = bin_files.iter().map(|b| b.size).sum();

        Ok(Self {
            installer_hash,
            output_dir: output_dir.to_path_buf(),
            completed_bins: HashMap::new(),
            current_bin: None,
            current_offset: 0,
            total_bins,
            total_bytes,
            extracted_bytes: 0,
        })
    }

    pub fn load(state_file: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(state_file)
            .context("Failed to read state file")?;
        let state: ExtractionState = serde_json::from_str(&content)
            .context("Failed to parse state file")?;
        Ok(state)
    }

    pub fn save(&self, state_file: &Path) -> Result<()> {
        if let Some(parent) = state_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize state")?;
        std::fs::write(state_file, content)
            .context("Failed to write state file")?;
        Ok(())
    }

    pub fn mark_bin_started(&mut self, bin_name: &str) {
        self.current_bin = Some(bin_name.to_string());
        self.current_offset = 0;
    }

    pub fn update_bin_progress(&mut self, extracted: u64) {
        self.current_offset = extracted;
        self.extracted_bytes = self.completed_bins.values()
            .map(|b| b.extracted_size)
            .sum::<u64>() + extracted;
    }

    pub fn mark_bin_completed(&mut self, bin_name: &str, size: u64, checksum: Option<String>, output_files: Vec<String>) {
        self.completed_bins.insert(bin_name.to_string(), BinState {
            name: bin_name.to_string(),
            size,
            extracted_size: size,
            completed: true,
            checksum,
            output_files,
        });
        self.current_bin = None;
        self.current_offset = 0;
        self.extracted_bytes = self.completed_bins.values()
            .map(|b| b.extracted_size)
            .sum();
    }

    pub fn is_bin_completed(&self, bin_name: &str) -> bool {
        self.completed_bins.get(bin_name).map(|b| b.completed).unwrap_or(false)
    }

    pub fn get_progress(&self) -> (u64, u64, f32) {
        let extracted = self.extracted_bytes;
        let total = self.total_bytes;
        let percent = if total > 0 { (extracted as f32 / total as f32) * 100.0 } else { 0.0 };
        (extracted, total, percent)
    }

    pub fn get_bin_progress(&self, bin_name: &str) -> Option<(u64, u64, f32)> {
        self.completed_bins.get(bin_name).map(|b| {
            let extracted = if self.current_bin.as_deref() == Some(bin_name) {
                self.current_offset
            } else {
                b.extracted_size
            };
            let percent = if b.size > 0 { (extracted as f32 / b.size as f32) * 100.0 } else { 100.0 };
            (extracted, b.size, percent)
        })
    }
}

fn compute_installer_hash(installer_dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    
    // Hash setup.exe and all .bin files
    let mut files: Vec<_> = std::fs::read_dir(installer_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name == "setup.exe" || (name.starts_with("fg-") && name.ends_with(".bin"))
        })
        .collect();
    
    files.sort_by_key(|e| e.file_name());
    
    for entry in files {
        let content = std::fs::read(entry.path())?;
        hasher.update(&content);
    }
    
    Ok(hex::encode(hasher.finalize()))
}

pub fn get_state_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("game-extractor")
}