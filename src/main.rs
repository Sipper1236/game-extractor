mod config;
mod innoextract;
mod layout;
mod state;
mod verify;
mod backend;
mod freearc_parser;

use std::path::PathBuf;
use std::sync::Arc;
use clap::{Parser, Subcommand};
use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use tokio::sync::mpsc;
use sha2::{Sha256, Digest};
use crate::backend::ExtractionBackend;
use crate::config::InstallerConfig;

#[derive(Parser)]
#[command(name = "game-extractor", version, about = "Native Linux game installer extractor for FitGirl repacks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract a FitGirl repack installer
    Install {
        /// Path to the installer directory (containing setup.exe and .bin files)
        #[arg(value_name = "INSTALLER_DIR")]
        installer_dir: PathBuf,

        /// Output directory for extracted game files
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,

        /// Skip MD5 verification
        #[arg(long)]
        no_verify: bool,

        /// Resume from previous extraction state
        #[arg(long)]
        resume: bool,

        /// Select specific content to extract (comma-separated: main,optional,selective-<lang>)
        #[arg(long, value_name = "CONTENT")]
        select: Option<String>,

        /// List available content without extracting
        #[arg(long)]
        list: bool,

        /// Maximum concurrent extractions (default: 2, env: GAME_EXTRACTOR_JOBS)
        #[arg(long, value_name = "N")]
        jobs: Option<usize>,
    },

    /// Verify MD5 checksums of installer files
    Verify {
        /// Path to the installer directory
        #[arg(value_name = "INSTALLER_DIR")]
        installer_dir: PathBuf,
    },

    /// Show information about the installer
    Info {
        /// Path to the installer directory
        #[arg(value_name = "INSTALLER_DIR")]
        installer_dir: PathBuf,
    },

    /// Test FreeArc archive parsing
    ParseArc {
        /// Path to .bin file
        #[arg(value_name = "BIN_FILE")]
        bin_file: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install { installer_dir, output, no_verify, resume, select, list, jobs } => {
            run_install(installer_dir, output, no_verify, resume, select, list, jobs).await
        }
        Commands::Verify { installer_dir } => {
            run_verify(installer_dir).await
        }
        Commands::Info { installer_dir } => {
            run_info(installer_dir).await
        }
        Commands::ParseArc { bin_file } => {
            run_parse_arc(bin_file).await
        }
    }
}

async fn run_install(
    installer_dir: PathBuf,
    output: Option<PathBuf>,
    no_verify: bool,
    resume: bool,
    select: Option<String>,
    list_only: bool,
    jobs: Option<usize>,
) -> Result<()> {
    println!("🔍 Scanning installer directory: {}", installer_dir.display());

    let layout = layout::scan_installer_dir(&installer_dir)?;
    println!("Found {} bin files ({:.2} GB total)",
        layout.bin_files.len(),
        layout.total_size as f64 / 1_073_741_824.0
    );

    // Apply selection filter
    let selection = if let Some(s) = select {
        layout::Selection::from_string(&s)?
    } else {
        layout::Selection::default()
    };

    // Clone the bins we need to avoid lifetime issues
    let bins_to_extract: Vec<layout::BinFile> = layout.bin_files.iter()
        .filter(|b| selection.should_extract(b))
        .cloned()
        .collect();

    if list_only {
        println!("\n📦 Available content:");
        for bin in &layout.bin_files {
            let selected = selection.should_extract(bin);
            let marker = if selected { "✓" } else { " " };
            let lang = bin.language.as_deref().unwrap_or("");
            println!("  [{}] {} ({:.2} GB) {:?} {}",
                marker, bin.name, bin.size as f64 / 1_073_741_824.0,
                bin.content_type, lang);
        }
        return Ok(());
    }

    if bins_to_extract.is_empty() {
        println!("No content selected for extraction");
        return Ok(());
    }

    println!("\n📋 Selected for extraction:");
    for bin in &bins_to_extract {
        let lang = bin.language.as_deref().unwrap_or("");
        println!("  - {} ({:.2} GB) {:?} {}",
            bin.name, bin.size as f64 / 1_073_741_824.0,
            bin.content_type, lang);
    }

    // MD5 verification
    if !no_verify {
        if let Some(md5_file) = &layout.md5_file {
            println!("\n🔐 Verifying MD5 checksums...");
            let results = verify::verify_installer_md5(&installer_dir, md5_file).await?;
            let mut all_ok = true;
            for (file, ok) in results {
                let status = if ok { "✓" } else { "✗" };
                println!("  [{}] {}", status, file);
                if !ok { all_ok = false; }
            }
            if !all_ok {
                anyhow::bail!("MD5 verification failed. Use --no-verify to skip.");
            }
            println!("  All checksums verified ✓");
        }
    }

    // Determine output directory
    let output_dir = output.unwrap_or_else(|| {
        // Ask user interactively
        println!("\n📁 Enter output directory for extracted game files:");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        PathBuf::from(input.trim())
    });

    tokio::fs::create_dir_all(&output_dir).await?;

    // Initialize backends with max_concurrent for Wine pool
    let max_concurrent = jobs
        .or_else(|| std::env::var("GAME_EXTRACTOR_JOBS").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(2); // Default to 2 concurrent extractions
    
    let native_backend = backend::NativeBackend::new();
    let mut wine_backend = backend::WineBackend::new(max_concurrent)?;
    wine_backend.set_installer_dir(&installer_dir);

    // Load config only if native backend can handle at least one bin
    let config = if bins_to_extract.iter().any(|b| native_backend.can_handle(b, &InstallerConfig::default())) {
        let setup_exe = installer_dir.join(&layout.setup_exe);
        let extract_dir = tempfile::tempdir()?;
        println!("\n📦 Extracting installer metadata with innoextract (for native backend)...");
        innoextract::extract_setup(&setup_exe, extract_dir.path()).await?;
        config::load_installer_config(extract_dir.path())?
    } else {
        println!("\n📦 Using fitgirl-extractor for all bins (no native backend needed)");
        InstallerConfig::default()
    };

    println!("\n🔧 Available backends:");
    let native_tools = native_backend.get_required_tools();
    let native_tools_str = if native_tools.is_empty() { "no tools found".to_string() } else { native_tools.join(", ") };
    let wine_tools = wine_backend.get_required_tools();
    let wine_tools_str = if wine_tools.is_empty() { "wine not available".to_string() } else { format!("{} prefixes", max_concurrent) };
    println!("  - native: {}", native_tools_str);
    println!("  - wine: {}", wine_tools_str);

    // Wrap backends in Arc for sharing across tasks
    let native_backend = Arc::new(native_backend);
    let wine_backend = Arc::new(wine_backend);
    let config = Arc::new(config);

    // Load or create extraction state
    let state_dir = state::get_state_dir();
    let mut hasher = Sha256::new();
    hasher.update(installer_dir.to_string_lossy().as_bytes());
    let installer_hash = hex::encode(hasher.finalize());
    let state_file = state_dir.join(format!("{}.json", installer_hash));

    let mut extraction_state = if resume && state_file.exists() {
        println!("📂 Resuming from previous state...");
        state::ExtractionState::load(&state_file)?
    } else {
        state::ExtractionState::new(&installer_dir, &output_dir, &layout.bin_files)?
    };

    println!("\n🚀 Starting multi-threaded extraction (max {} concurrent)...", max_concurrent);

    // Progress tracking
    let multi = MultiProgress::new();
    let overall_pb = multi.add(ProgressBar::new(extraction_state.total_bytes));
    overall_pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .progress_chars("##-"));
    overall_pb.set_message("Overall");

    // Extract each bin with concurrency control
    // Wine backend manages its own pool internally
    let mut join_handles = Vec::new();

    for bin in bins_to_extract {
        if extraction_state.is_bin_completed(&bin.name) {
            println!("⏭️  Skipping {} (already completed)", bin.name);
            continue;
        }

        let native_backend = native_backend.clone();
        let wine_backend = wine_backend.clone();
        let config = config.clone();
        let output_dir = output_dir.clone();
        let state_file = state_file.clone();
        let multi = multi.clone();
        let overall_pb = overall_pb.clone();
        
        let handle = tokio::spawn(async move {
            println!("\n📦 Extracting {}...", bin.name);
            
            // We need to track progress per bin
            let bin_pb = multi.add(ProgressBar::new(bin.size));
            bin_pb.set_style(ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("##-"));
            bin_pb.set_message(bin.name.clone());

            let (progress_tx, mut progress_rx) = mpsc::channel(100);

            // Choose backend
            let use_native = native_backend.can_handle(&bin, &config);
            let backend: Arc<dyn ExtractionBackend> = if use_native {
                native_backend.clone()
            } else if wine_backend.can_handle(&bin, &config) {
                wine_backend.clone()
            } else {
                anyhow::bail!("No backend can handle {} (unsupported codecs)", bin.name);
            };

            println!("   Using backend: {}", backend.name());

            // Spawn extraction task
            let bin_clone = bin.clone();
            let output_dir_clone = output_dir.clone();
            let config_clone = config.clone();
            let backend_clone = backend.clone();
            
            let extract_handle = tokio::spawn(async move {
                backend_clone.extract(&bin_clone, &config_clone, &output_dir_clone, Some(progress_tx)).await
            });

            // Update progress
            while let Some(progress) = progress_rx.recv().await {
                bin_pb.set_position(progress);
            }

            let result = extract_handle.await??;
            bin_pb.finish_with_message("Done");
            
            Ok::<_, anyhow::Error>((bin.name, bin.size, result))
        });
        
        join_handles.push(handle);
    }

    // Wait for all extractions to complete
    for handle in join_handles {
        let (bin_name, bin_size, _result) = handle.await??;
        extraction_state.mark_bin_completed(&bin_name, bin_size, None, vec![]);
        extraction_state.save(&state_file)?;
        overall_pb.set_position(extraction_state.extracted_bytes);
    }

    overall_pb.finish_with_message("Complete");
    println!("\n✅ Extraction complete! Game files in: {}", output_dir.display());

    // Clean up state file on success
    let _ = std::fs::remove_file(&state_file);

    Ok(())
}

async fn run_verify(installer_dir: PathBuf) -> Result<()> {
    println!("🔐 Verifying MD5 checksums for: {}", installer_dir.display());

    let layout = layout::scan_installer_dir(&installer_dir)?;
    if let Some(md5_file) = layout.md5_file {
        let results = verify::verify_installer_md5(&installer_dir, &md5_file).await?;
        let mut all_ok = true;
        for (file, ok) in results {
            let status = if ok { "✓" } else { "✗" };
            println!("  [{}] {}", status, file);
            if !ok { all_ok = false; }
        }
        if all_ok {
            println!("\n✅ All checksums verified");
        } else {
            anyhow::bail!("Some checksums failed");
        }
    } else {
        println!("No MD5 file found");
    }
    Ok(())
}

async fn run_info(installer_dir: PathBuf) -> Result<()> {
    println!("ℹ️  Installer info for: {}", installer_dir.display());

    let layout = layout::scan_installer_dir(&installer_dir)?;
    println!("Setup: {}", layout.setup_exe);
    println!("Total bins: {}", layout.bin_files.len());
    println!("Total size: {:.2} GB", layout.total_size as f64 / 1_073_741_824.0);
    println!("MD5 file: {:?}", layout.md5_file);

    println!("\nBin files:");
    for bin in &layout.bin_files {
        let lang = bin.language.as_deref().unwrap_or("");
        println!("  - {} ({:.2} GB) {:?} {}",
            bin.name, bin.size as f64 / 1_073_741_824.0,
            bin.content_type, lang);
    }

    Ok(())
}

async fn run_parse_arc(bin_file: PathBuf) -> Result<()> {
    println!("🔍 Parsing FreeArc archive: {}", bin_file.display());

    match freearc_parser::parse_freearc_archive(&bin_file) {
        Ok(files) => {
            println!("Found {} files:", files.len());
            for (i, file) in files.iter().enumerate() {
                println!("  {}. {} ({} bytes, method: {}, crc32: {:08x})",
                    i + 1, file.name, file.size, file.method, file.crc32);
            }
        }
        Err(e) => {
            eprintln!("Failed to parse archive: {}", e);
            anyhow::bail!(e);
        }
    }
    Ok(())
}