//! Entry point for the media-forge CLI tool.
//!
//! Media-Forge provides high-performance batch media conversion capabilities,
//! utilizing multi-threading for image processing and NVIDIA hardware acceleration
//! for video encoding.

use anyhow::Result;
use clap::{Parser, Subcommand};
use media_forge::{ProcessManager, image, video};
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Media-Forge: High-performance batch media conversion tool.
#[derive(Parser)]
#[command(name = "media-forge")]
#[command(version)]
#[command(author = "Media-Forge Contributors")]
#[command(about = "High-performance batch media conversion tool")]
#[command(
    long_about = "Media-Forge is a CLI tool for batch media conversion on Linux.\n\n    Features:\n  \n  - Convert images to AVIF/WebP with configurable quality\n  \n  - Encode videos to AV1 using NVIDIA CUDA acceleration\n  \n  - Create CBZ comic book archives from image folders\n\n    Use 'media-forge <command> --help' for detailed command information."
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

    /// Encode videos to AV1 using NVIDIA hardware acceleration
    ///
    /// Uses FFmpeg with NVIDIA NVENC for hardware-accelerated AV1 encoding.
    /// Requires an NVIDIA GPU with NVENC support (GTX 10-series or newer).
    /// Automatically skips videos already encoded in AV1.
    #[command(name = "video", visible_alias = "vid")]
    Video(video::VideoArgs),

    /// Compare video quality using VMAF
    ///
    /// Analyzes the quality of an encoded video compared to its original source.
    /// Provides mean, min, and max VMAF scores with a quality rating.
    /// Requires FFmpeg with libvmaf support.
    #[command(name = "vmaf", visible_alias = "qvid")]
    Quality(video::QualityArgs),
}

/// Parses command-line arguments and routes execution to the appropriate subcommand.
fn main() -> Result<()> {
    ctrlc::try_set_handler(move || {
        eprintln!("\n\x1b[31m[Interrupt] Shutting down and cleaning up child processes...\x1b[0m");
        ProcessManager::kill_all();
        std::process::exit(130);
    })?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Image(args) => image::run(args),
        Commands::Archive(args) => image::run_archive(args),
        Commands::ImageQuality(args) => image::run_quality(args),
        Commands::Video(args) => video::run(args),
        Commands::Quality(args) => video::run_quality(args),
    }
}
