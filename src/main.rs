//! Entry point for the media-forge CLI tool.
//!
//! Media-Forge provides high-performance image conversion and archive creation.

use anyhow::Result;
use clap::{Parser, Subcommand};
use media_forge::{SHUTDOWN, image};
use mimalloc::MiMalloc;
use std::sync::atomic::Ordering;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Media-Forge: High-performance batch media conversion tool.
#[derive(Parser)]
#[command(name = "media-forge")]
#[command(version)]
#[command(author = "Media-Forge Contributors")]
#[command(about = "High-performance batch media conversion tool")]
#[command(
    long_about = "Media-Forge is a CLI tool for image conversion and archive creation on Linux.\n\n    Features:\n  \n  - Convert images to AVIF/WebP with configurable quality\n  \n  - Compare image quality with SSIMULACRA2\n  \n  - Create CBZ comic book archives from image folders\n\n    Use 'media-forge <command> --help' for detailed command information."
)]
#[command(propagate_version = true)]
struct Cli {
    /// Command to execute.
    #[command(subcommand)]
    command: Commands,
}

/// Supported subcommands for media processing.
#[derive(Subcommand)]
enum Commands {
    /// Convert images to modern formats (AVIF, WebP)
    ///
    /// Batch convert images with configurable quality and compression speed.
    /// Supports direct files and images inside ZIP/CBZ archives.
    /// Preserves directory structure and original modification times.
    #[command(name = "image", visible_alias = "img")]
    Image(image::ImageArgs),

    /// Create CBZ comic book archives from image folders
    ///
    /// Scans directories for image folders and creates properly formatted
    /// CBZ archives with natural sorting.
    /// Supports dry-run mode to preview operations before execution.
    #[command(name = "archive", visible_alias = "zip")]
    Archive(image::ArchiveArgs),

    /// Compare image quality using SSIMULACRA2
    ///
    /// Analyzes the quality of a distorted image compared to its original source.
    /// Provides a score from 0-100 with a quality rating.
    #[command(name = "simulacra", visible_alias = "qimg")]
    ImageQuality(image::QualityArgs),
}

/// Parses command-line arguments and routes execution to the appropriate subcommand.
fn main() -> Result<()> {
    ctrlc::try_set_handler(move || {
        eprintln!("\n\x1b[31m[Interrupt] Shutting down...\x1b[0m");
        SHUTDOWN.store(true, Ordering::SeqCst);
        std::process::exit(130);
    })?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Image(args) => image::run(args),
        Commands::Archive(args) => image::run_archive(args),
        Commands::ImageQuality(args) => image::run_quality(args),
    }
}
