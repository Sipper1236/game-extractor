use std::collections::HashMap;
use std::path::Path;
use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArcIni {
    #[serde(flatten)]
    pub external_compressors: HashMap<String, ExternalCompressor>,
    pub compression: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalCompressor {
    #[serde(default)]
    pub header: Option<i32>,
    #[serde(default)]
    pub solid: Option<i32>,
    #[serde(default)]
    pub mem: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub cmem: Option<String>,
    #[serde(default)]
    pub dmem: Option<String>,
    #[serde(rename = "unpackcmd")]
    pub unpack_cmd: Option<String>,
    #[serde(rename = "packcmd")]
    pub pack_cmd: Option<String>,
    #[serde(default)]
    pub datafile: Option<String>,
    #[serde(default)]
    pub packedfile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClsIni {
    #[serde(flatten)]
    pub plugins: HashMap<String, ClsPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClsPlugin {
    #[serde(default)]
    pub bufsize: Option<String>,
    #[serde(rename = "transfer_ReadBufSize", default)]
    pub transfer_read_buf_size: Option<String>,
    #[serde(rename = "transfer_WriteBufSize", default)]
    pub transfer_write_buf_size: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(rename = "MaxMemoryUsage", default)]
    pub max_memory_usage: Option<String>,
    #[serde(rename = "MaxThreadsUsage", default)]
    pub max_threads_usage: Option<String>,
    #[serde(rename = "ldmfMaxMemoryUsage", default)]
    pub ldmf_max_memory_usage: Option<String>,
    #[serde(rename = "ldmfTempPath", default)]
    pub ldmf_temp_path: Option<String>,
    #[serde(rename = "ldmfDeleteTmp", default)]
    pub ldmf_delete_tmp: Option<String>,
    #[serde(default)]
    pub temp_path: Option<String>,
    #[serde(rename = "TmpPath", default)]
    pub tmp_path: Option<String>,
    #[serde(default)]
    pub threads: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstallerConfig {
    pub arc_ini: ArcIni,
    pub cls_ini: ClsIni,
    pub records_inf: Vec<RecordEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordEntry {
    pub field1: u32,
    pub field2: u32,
    pub field3: u32,
    pub field4: u32,
}

pub fn parse_arc_ini(content: &str) -> Result<ArcIni> {
    let mut external_compressors = HashMap::new();
    let mut compression = None;
    let mut current_section = String::new();
    let mut current_compressor: Option<ExternalCompressor> = None;
    let mut in_external = false;
    let mut in_compression = false;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            // Save previous section
            if in_external {
                if let Some(comp) = current_compressor.take() {
                    external_compressors.insert(current_section.clone(), comp);
                }
            }

            current_section = line[1..line.len()-1].to_string();
            
            if current_section.starts_with("External compressor:") {
                in_external = true;
                in_compression = false;
                current_compressor = Some(ExternalCompressor::default());
            } else if current_section == "Compression" {
                in_external = false;
                in_compression = true;
                compression = Some(HashMap::new());
            } else {
                in_external = false;
                in_compression = false;
            }
            continue;
        }

        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_lowercase();
            let value = line[eq_pos+1..].trim().to_string();

            if in_external {
                if let Some(ref mut comp) = current_compressor {
                    match key.as_str() {
                        "header" => comp.header = value.parse().ok(),
                        "solid" => comp.solid = value.parse().ok(),
                        "mem" => comp.mem = Some(value),
                        "default" => comp.default = Some(value),
                        "cmem" => comp.cmem = Some(value),
                        "dmem" => comp.dmem = Some(value),
                        "unpackcmd" => comp.unpack_cmd = Some(value),
                        "packcmd" => comp.pack_cmd = Some(value),
                        "datafile" => comp.datafile = Some(value),
                        "packedfile" => comp.packedfile = Some(value),
                        _ => {}
                    }
                }
            } else if in_compression {
                if let Some(ref mut comp_map) = compression {
                    comp_map.insert(key, value);
                }
            }
        }
    }

    // Save last section
    if in_external {
        if let Some(comp) = current_compressor {
            external_compressors.insert(current_section, comp);
        }
    }

    Ok(ArcIni {
        external_compressors,
        compression,
    })
}

pub fn parse_cls_ini(content: &str) -> Result<ClsIni> {
    let mut plugins = HashMap::new();
    let mut current_section = String::new();
    let mut current_plugin: Option<ClsPlugin> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            if let Some(plugin) = current_plugin.take() {
                plugins.insert(current_section.clone(), plugin);
            }
            current_section = line[1..line.len()-1].to_string();
            current_plugin = Some(ClsPlugin::default());
            continue;
        }

        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim();
            let value = line[eq_pos+1..].trim().to_string();

            if let Some(ref mut plugin) = current_plugin {
                match key {
                    "Bufsize" => plugin.bufsize = Some(value),
                    "transfer_ReadBufSize" => plugin.transfer_read_buf_size = Some(value),
                    "transfer_WriteBufSize" => plugin.transfer_write_buf_size = Some(value),
                    "Memory" => plugin.memory = Some(value),
                    "MaxMemoryUsage" => plugin.max_memory_usage = Some(value),
                    "MaxThreadsUsage" => plugin.max_threads_usage = Some(value),
                    "ldmfMaxMemoryUsage" => plugin.ldmf_max_memory_usage = Some(value),
                    "ldmfTempPath" => plugin.ldmf_temp_path = Some(value),
                    "ldmfDeleteTmp" => plugin.ldmf_delete_tmp = Some(value),
                    "TempPath" => plugin.temp_path = Some(value),
                    "TmpPath" => plugin.tmp_path = Some(value),
                    "Threads" => plugin.threads = Some(value),
                    _ => {}
                }
            }
        }
    }

    if let Some(plugin) = current_plugin {
        plugins.insert(current_section, plugin);
    }

    Ok(ClsIni { plugins })
}

pub fn parse_records_inf(content: &str) -> Result<Vec<RecordEntry>> {
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            entries.push(RecordEntry {
                field1: parts[0].parse().unwrap_or(0),
                field2: parts[1].parse().unwrap_or(0),
                field3: parts[2].parse().unwrap_or(0),
                field4: parts[3].parse().unwrap_or(0),
            });
        }
    }
    Ok(entries)
}

pub fn load_installer_config(extract_dir: &Path) -> Result<InstallerConfig> {
    let arc_ini_path = extract_dir.join("arc.ini");
    let cls_ini_path = extract_dir.join("CLS.ini");
    let records_path = extract_dir.join("records.inf");

    let arc_content = std::fs::read_to_string(&arc_ini_path)
        .context("Failed to read arc.ini")?;
    let cls_content = std::fs::read_to_string(&cls_ini_path)
        .context("Failed to read CLS.ini")?;
    let records_content = std::fs::read_to_string(&records_path)
        .context("Failed to read records.inf")?;

    let arc_ini = parse_arc_ini(&arc_content)?;
    let cls_ini = parse_cls_ini(&cls_content)?;
    let records_inf = parse_records_inf(&records_content)?;

    Ok(InstallerConfig {
        arc_ini,
        cls_ini,
        records_inf,
    })
}