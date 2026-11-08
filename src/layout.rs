use std::path::Path;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub content_type: ContentType,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType {
    Main,
    Optional,
    Selective,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallerLayout {
    pub setup_exe: String,
    pub bin_files: Vec<BinFile>,
    pub md5_file: Option<String>,
    pub total_size: u64,
}

pub fn scan_installer_dir(installer_dir: &Path) -> Result<InstallerLayout> {
    let mut bin_files = Vec::new();
    let mut setup_exe = None;
    let mut md5_file = None;
    let mut total_size = 0u64;

    // First pass: find .bin files and setup.exe in root
    for entry in WalkDir::new(installer_dir).max_depth(1) {
        let entry = entry?;
        let path = entry.path();
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if file_name.eq_ignore_ascii_case("setup.exe") {
            setup_exe = Some(file_name.clone());
        } else if file_name.starts_with("fg-") && file_name.ends_with(".bin") {
            let metadata = entry.metadata()?;
            let size = metadata.len();
            total_size += size;

            let content_type = classify_bin_file(&file_name);
            let language = extract_language(&file_name);

            bin_files.push(BinFile {
                name: file_name.clone(),
                path: path.to_string_lossy().to_string(),
                size,
                content_type,
                language,
            });
        }
    }

    // Second pass: find MD5 file anywhere in the installer directory
    for entry in WalkDir::new(installer_dir) {
        let entry = entry?;
        let path = entry.path();
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if file_name.eq_ignore_ascii_case("fitgirl-bins.md5") || file_name.ends_with(".md5") {
            // Store relative path from installer_dir
            md5_file = Some(path.strip_prefix(installer_dir)?.to_string_lossy().to_string());
            break;
        }
    }

    // Sort bin files: main first (by number), then optional, then selective
    bin_files.sort_by(|a, b| {
        let type_order = |t: &ContentType| match t {
            ContentType::Main => 0,
            ContentType::Optional => 1,
            ContentType::Selective => 2,
        };
        type_order(&a.content_type).cmp(&type_order(&b.content_type))
            .then_with(|| extract_bin_number(&a.name).cmp(&extract_bin_number(&b.name)))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(InstallerLayout {
        setup_exe: setup_exe.unwrap_or_default(),
        bin_files,
        md5_file,
        total_size,
    })
}

fn classify_bin_file(name: &str) -> ContentType {
    let lower = name.to_lowercase();
    if lower.contains("optional") {
        ContentType::Optional
    } else if lower.contains("selective") {
        ContentType::Selective
    } else {
        ContentType::Main
    }
}

fn extract_language(name: &str) -> Option<String> {
    // fg-selective-english-vo.bin -> english
    if let Some(start) = name.find("selective-") {
        let rest = &name[start + 10..]; // skip "selective-"
        if let Some(end) = rest.find("-vo") {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(start) = name.find("optional-") {
        let rest = &name[start + 9..]; // skip "optional-"
        if let Some(end) = rest.find(".bin") {
            return Some(rest[..end].to_string());
        }
    }
    None
}

fn extract_bin_number(name: &str) -> u32 {
    // fg-01.bin -> 1
    // fg-84.bin -> 84
    if let Some(start) = name.find("fg-") {
        let rest = &name[start + 3..];
        if let Some(end) = rest.find(".bin") {
            let num_str = &rest[..end];
            return num_str.parse().unwrap_or(999);
        }
    }
    999
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub include_main: bool,
    pub include_optional: Vec<String>,  // specific optional names
    pub include_selective: Vec<String>, // language codes
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            include_main: false,
            include_optional: Vec::new(),
            include_selective: Vec::new(),
        }
    }
}

impl Selection {
    pub fn from_string(s: &str) -> Result<Self> {
        let mut selection = Selection::default();
        
        for part in s.split(',') {
            let part = part.trim().to_lowercase();
            match part.as_str() {
                "main" => selection.include_main = true,
                "optional" => selection.include_optional.push("all".to_string()),
                s if s.starts_with("optional-") => {
                    selection.include_optional.push(s["optional-".len()..].to_string());
                }
                s if s.starts_with("selective-") || s.starts_with("vo-") => {
                    let lang = s.strip_prefix("selective-").or_else(|| s.strip_prefix("vo-")).unwrap_or(s);
                    selection.include_selective.push(lang.to_string());
                }
                "all" => {
                    selection.include_main = true;
                    selection.include_optional.push("all".to_string());
                    selection.include_selective.push("all".to_string());
                }
                _ => anyhow::bail!("Unknown selection: {}", part),
            }
        }
        Ok(selection)
    }

    pub fn should_extract(&self, bin: &BinFile) -> bool {
        match bin.content_type {
            ContentType::Main => self.include_main,
            ContentType::Optional => {
                self.include_optional.contains(&"all".to_string()) ||
                bin.language.as_ref().map(|l| self.include_optional.contains(l)).unwrap_or(false)
            }
            ContentType::Selective => {
                self.include_selective.contains(&"all".to_string()) ||
                bin.language.as_ref().map(|l| self.include_selective.contains(l)).unwrap_or(false)
            }
        }
    }
}