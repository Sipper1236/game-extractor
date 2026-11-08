use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use anyhow::{Result, Context};
use md5::Context as Md5Context;
use tokio::sync::Semaphore;
use tokio::task;
use futures::future::join_all;

pub fn parse_md5_file(md5_path: &Path) -> Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(md5_path)
        .context("Failed to read MD5 file")?;
    
    let mut map = HashMap::new();
    
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let hash = parts[0].to_lowercase();
            let file_name = parts[1].trim_start_matches('*');
            let file_name = file_name.trim_start_matches("..\\").trim_start_matches("../");
            map.insert(file_name.to_string(), hash);
        }
    }
    
    Ok(map)
}

async fn verify_single_file(file_path: std::path::PathBuf, expected_hash: String) -> Result<(String, bool)> {
    let file_name = file_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    
    let file = tokio::fs::File::open(&file_path).await
        .context("Failed to open file for verification")?;
    
    let mut reader = tokio::io::BufReader::new(file);
    let mut hasher = Md5Context::new();
    let mut buffer = vec![0u8; 1024 * 1024]; // 1MB buffer for better performance
    
    loop {
        let n = tokio::io::AsyncReadExt::read(&mut reader, &mut buffer).await
            .context("Failed to read file for hashing")?;
        if n == 0 {
            break;
        }
        hasher.consume(&buffer[..n]);
    }
    
    let computed = format!("{:x}", hasher.compute());
    Ok((file_name, computed.eq_ignore_ascii_case(&expected_hash)))
}

pub async fn verify_installer_md5(installer_dir: &Path, md5_file: &str) -> Result<Vec<(String, bool)>> {
    let md5_path = installer_dir.join(md5_file);
    let expected_hashes = parse_md5_file(&md5_path)?;
    
    // Limit concurrent verifications to avoid disk thrashing
    // 2-3 concurrent reads is optimal for HDD; SSD can handle more
    let semaphore = Arc::new(Semaphore::new(2));
    let mut tasks = Vec::new();
    
    for (file_name, expected_hash) in expected_hashes {
        let file_path = installer_dir.join(&file_name);
        let semaphore = semaphore.clone();
        
        if file_path.exists() {
            tasks.push(task::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                verify_single_file(file_path, expected_hash).await
            }));
        } else {
            // File doesn't exist, add failed result immediately
            tasks.push(task::spawn(async move {
                Ok((file_name, false))
            }));
        }
    }
    
    // Wait for all verifications to complete
    let results = join_all(tasks).await;
    
    let mut final_results = Vec::new();
    for result in results {
        match result {
            Ok(Ok((name, verified))) => final_results.push((name, verified)),
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(anyhow::anyhow!("Task join error: {}", e)),
        }
    }
    
    Ok(final_results)
}