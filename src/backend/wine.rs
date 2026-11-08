use std::path::{Path, PathBuf};
use std::sync::Arc;
use anyhow::{Result, Context};
use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore};
use crate::backend::ExtractionBackend;
use crate::config::InstallerConfig;
use crate::layout::BinFile;

/// CPU limiter configuration
#[derive(Debug, Clone)]
pub struct CpuLimiter {
    /// Use cpulimit to restrict CPU percentage (e.g., 50 = 50% of one core)
    pub cpulimit_percent: Option<u32>,
    /// Nice value (-20 to 19, higher = lower priority)
    pub nice: Option<i32>,
}

impl CpuLimiter {
    pub fn from_env() -> Self {
        Self {
            cpulimit_percent: std::env::var("GAME_EXTRACTOR_CPU_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok()),
            nice: std::env::var("GAME_EXTRACTOR_NICE")
                .ok()
                .and_then(|v| v.parse().ok()),
        }
    }

    /// Wrap a command with CPU limiting
    pub fn wrap_command(&self, cmd: &mut Command) {
        // Apply nice value if set
        if let Some(nice_val) = self.nice {
            // nice is applied via the command wrapper
            // We'll prepend "nice -n {nice_val}" to the command
        }
        // cpulimit is applied at runtime by wrapping the spawned process
    }

    /// Build the command prefix for CPU limiting
    pub fn command_prefix(&self) -> Vec<String> {
        let mut prefix = Vec::new();
        if let Some(pct) = self.cpulimit_percent {
            // Check if cpulimit is available
            if which::which("cpulimit").is_ok() {
                prefix.push("cpulimit".to_string());
                prefix.push("--limit".to_string());
                prefix.push(pct.to_string());
                prefix.push("--".to_string());
            }
        }
        if let Some(nice_val) = self.nice {
            prefix.push("nice".to_string());
            prefix.push("-n".to_string());
            prefix.push(nice_val.to_string());
        }
        prefix
    }
}

/// Pool of Wine prefixes for parallel extraction
pub struct WinePrefixPool {
    prefixes: Vec<PathBuf>,
    available: Arc<Mutex<Vec<usize>>>,
    semaphore: Arc<Semaphore>,
}

impl WinePrefixPool {
    pub fn new(count: usize, base_dir: PathBuf) -> Result<Self> {
        let mut prefixes = Vec::with_capacity(count);
        for i in 0..count {
            prefixes.push(base_dir.join(format!("wine-prefix-{}", i)));
        }
        
        let available: Vec<usize> = (0..count).collect();
        
        Ok(Self {
            prefixes,
            available: Arc::new(Mutex::new(available)),
            semaphore: Arc::new(Semaphore::new(count)),
        })
    }

    /// Acquire a Wine prefix for use
    pub async fn acquire(&self) -> Result<WinePrefixGuard> {
        let permit = self.semaphore.acquire().await
            .context("Failed to acquire Wine prefix permit")?;
        
        let idx = {
            let mut available = self.available.lock().await;
            available.pop()
                .context("No Wine prefix available")?
        };
        
        Ok(WinePrefixGuard {
            pool: self,
            prefix: self.prefixes[idx].clone(),
            idx,
            _permit: permit,
        })
    }

    async fn release(&self, idx: usize) {
        self.available.lock().await.push(idx);
    }

    pub async fn ensure_all(&self) -> Result<()> {
        for prefix in &self.prefixes {
            if !prefix.exists() {
                tokio::fs::create_dir_all(prefix).await?;
                
                let status = Command::new("wine")
                    .env("WINEPREFIX", prefix)
                    .arg("wineboot")
                    .arg("--init")
                    .status()
                    .await
                    .context("Failed to run wineboot")?;
                
                if !status.success() {
                    anyhow::bail!("wineboot failed for {}", prefix.display());
                }
            }
        }
        Ok(())
    }
}

pub struct WinePrefixGuard<'a> {
    pool: &'a WinePrefixPool,
    prefix: PathBuf,
    idx: usize,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl<'a> Drop for WinePrefixGuard<'a> {
    fn drop(&mut self) {
        // Can't spawn async in Drop, use a synchronous approach
        // The semaphore permit is automatically released when _permit drops
        // We just need to return the index to the available list
        // Use a blocking approach since we're in Drop
        let pool = self.pool;
        let idx = self.idx;
        // Try to lock and release - if it fails, the prefix will be leaked but semaphore is released
        if let Ok(mut available) = pool.available.try_lock() {
            available.push(idx);
        }
    }
}

impl<'a> WinePrefixGuard<'a> {
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }
}

pub struct WineBackend {
    pub fitgirl_extractor: PathBuf,
    pub pool: Arc<WinePrefixPool>,
    pub installer_dir: PathBuf,
    pub cpu_limiter: CpuLimiter,
}

impl WineBackend {
    pub fn new(max_parallel: usize) -> Result<Self> {
        let fitgirl_extractor = PathBuf::from("/tmp/fitgirl-extractor.exe");
        if !fitgirl_extractor.exists() {
            anyhow::bail!("fitgirl-extractor.exe not found at {}", fitgirl_extractor.display());
        }

        let base_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("game-extractor");

        let pool = Arc::new(WinePrefixPool::new(max_parallel, base_dir)?);
        
        Ok(Self {
            fitgirl_extractor,
            pool,
            installer_dir: PathBuf::new(),
            cpu_limiter: CpuLimiter::from_env(),
        })
    }

    pub fn set_installer_dir(&mut self, dir: &Path) {
        self.installer_dir = dir.to_path_buf();
    }
}

#[async_trait]
impl ExtractionBackend for WineBackend {
    fn name(&self) -> &str {
        "wine (fitgirl-extractor, multi-prefix)"
    }

    fn can_handle(&self, _bin: &BinFile, _config: &InstallerConfig) -> bool {
        which::which("wine").is_ok() && self.fitgirl_extractor.exists()
    }

    async fn extract(&self, bin: &BinFile, _config: &InstallerConfig, output_dir: &Path, _progress_tx: Option<tokio::sync::mpsc::Sender<u64>>) -> Result<Vec<String>> {
        // Ensure all prefixes are initialized
        self.pool.ensure_all().await?;
        
        // Acquire a prefix for this extraction
        let guard = self.pool.acquire().await?;
        let prefix = guard.prefix().to_path_buf();
        
        tokio::fs::create_dir_all(output_dir).await?;
        
        let bin_name = &bin.name;
        
        // Build command with CPU limiting
        let cpu_prefix = self.cpu_limiter.command_prefix();
        let mut cmd = if cpu_prefix.is_empty() {
            Command::new("wine")
        } else {
            let mut c = Command::new(&cpu_prefix[0]);
            for arg in &cpu_prefix[1..] {
                c.arg(arg);
            }
            c.arg("wine");
            c
        };
        
        cmd.env("WINEPREFIX", &prefix)
           .env("WINEDEBUG", "-all")
           // GPU acceleration env vars for Wine
           .env("DXVK_HUD", "0")
           .env("VKD3D_CONFIG", "dxr11")
           .arg(&self.fitgirl_extractor)
           .arg(&self.installer_dir)
           .arg("-d")
           .arg(output_dir)
           .arg("--only")
           .arg(bin_name)
           .arg("--skip-bin-check")
           .arg("--skip-verify");

        eprintln!("Running on prefix {}: {} wine {} {} -d {} --only {} --skip-bin-check --skip-verify", 
            prefix.display(),
            cpu_prefix.join(" "),
            self.fitgirl_extractor.display(), 
            self.installer_dir.display(),
            output_dir.display(),
            bin_name);

        let mut child = cmd
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .context("Failed to spawn fitgirl-extractor")?;

        let status = child.wait().await.context("Failed to wait for fitgirl-extractor")?;
        
        if !status.success() {
            anyhow::bail!("fitgirl-extractor failed with exit code: {:?}", status.code());
        }

        Ok(vec![output_dir.to_string_lossy().to_string()])
    }

    fn get_required_tools(&self) -> Vec<String> {
        vec!["wine".to_string(), "fitgirl-extractor.exe".to_string()]
    }
}