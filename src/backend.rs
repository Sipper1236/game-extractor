pub mod native;
pub mod wine;

use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use crate::config::InstallerConfig;
use crate::layout::BinFile;

#[async_trait]
pub trait ExtractionBackend: Send + Sync {
    fn name(&self) -> &str;
    fn can_handle(&self, bin: &BinFile, config: &InstallerConfig) -> bool;
    async fn extract(&self, bin: &BinFile, config: &InstallerConfig, output_dir: &Path, progress_tx: Option<tokio::sync::mpsc::Sender<u64>>) -> Result<Vec<String>>;
    fn get_required_tools(&self) -> Vec<String>;
}

pub use native::NativeBackend;
pub use wine::WineBackend;