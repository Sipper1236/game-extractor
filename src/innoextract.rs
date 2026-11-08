use std::path::Path;
use anyhow::{Result, Context};
use tokio::process::Command;

pub async fn extract_setup(setup_exe: &Path, output_dir: &Path) -> Result<()> {
    // Create output directory
    tokio::fs::create_dir_all(output_dir).await
        .context("Failed to create output directory")?;

    // Run innoextract to extract the installer
    // -e: extract files without installing
    // -d: output directory
    // -q: quiet mode
    let status = Command::new("innoextract")
        .arg("-e")
        .arg("-d")
        .arg(output_dir)
        .arg(setup_exe)
        .status()
        .await
        .context("Failed to run innoextract. Is it installed?")?;

    if !status.success() {
        anyhow::bail!("innoextract failed with exit code: {:?}", status.code());
    }

    Ok(())
}

pub async fn list_setup_contents(setup_exe: &Path) -> Result<Vec<String>> {
    let output = Command::new("innoextract")
        .arg("-l")
        .arg(setup_exe)
        .output()
        .await
        .context("Failed to run innoextract -l")?;

    if !output.status.success() {
        anyhow::bail!("innoextract -l failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout.lines()
        .filter(|l| !l.is_empty() && !l.starts_with("Extracting"))
        .map(|l| l.trim().to_string())
        .collect();

    Ok(files)
}

pub async fn check_innoextract_available() -> bool {
    Command::new("innoextract")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}