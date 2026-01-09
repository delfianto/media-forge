use anyhow::Result;
use clap::{Parser, Subcommand};
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[derive(Parser)]
#[command(name = "media-forge")]
#[command(version)]
#[command(author = "Media-Forge Contributors")]
#[command(about = "High-performance batch media conversion tool")]
#[command(
    long_about = "Media-Forge is a CLI tool for batch media conversion on Linux.\n\n\
    Features:\n  \
    - Convert images to AVIF/WebP with configurable quality\n  \
    - Encode videos to AV1 using NVIDIA CUDA acceleration\n  \
    - Create CBZ comic book archives from image folders\n\n\
    Use 'media-forge <command> --help' for detailed command information."
)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Convert images to modern formats (AVIF, WebP)
    ///
    /// Batch convert images with configurable quality and compression speed.
    /// Supports direct files and images inside ZIP/CBZ archives.
    /// Preserves directory structure and original modification times.
    #[command(name = "image", alias = "img", visible_alias = "img")]
    Image(mf_image::ImageArgs),

    /// Create CBZ comic book archives from image folders
    ///
    /// Scans directories for image folders and creates properly formatted
    /// CBZ archives with automatic page numbering and natural sorting.
    /// Supports dry-run mode to preview operations before execution.
    #[command(name = "archive", alias = "arch", visible_alias = "arch")]
    Archive(mf_archive::ArchiveArgs),

    /// Encode videos to AV1 using NVIDIA hardware acceleration
    ///
    /// Uses FFmpeg with NVIDIA NVENC for hardware-accelerated AV1 encoding.
    /// Requires an NVIDIA GPU with NVENC support (GTX 10-series or newer).
    /// Automatically skips videos already encoded in AV1.
    #[command(name = "video", alias = "vid", visible_alias = "vid")]
    Video(mf_video::VideoArgs),
}

fn main() -> Result<()> {
    // Set up Ctrl-C handler to kill child processes and shutdown gracefully
    ctrlc::set_handler(move || {
        eprintln!("\n\x1b[31m[Interrupt] Shutting down and cleaning up child processes...\x1b[0m");
        mf_core::ProcessManager::kill_all();
        std::process::exit(130);
    })
    .expect("Error setting Ctrl-C handler");

    let cli = Cli::parse();

    match cli.command {
        Commands::Image(args) => mf_image::run(args),
        Commands::Archive(args) => mf_archive::run(args),
        Commands::Video(args) => mf_video::run(args),
    }
}
