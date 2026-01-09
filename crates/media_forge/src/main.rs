use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "media-forge")]
#[command(about = "Unified Media Forge CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Image processing (AVIF, WEBP)
    #[command(name = "image", alias = "img")]
    Image(mf_image::ImageArgs),

    /// Archive processing (CBZ)
    #[command(name = "archive", alias = "arch")]
    Archive(mf_archive::ArchiveArgs),

    /// Video processing (AV1)
    #[command(name = "video", alias = "vid")]
    Video(mf_video::VideoArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Image(args) => mf_image::run(args),
        Commands::Archive(args) => mf_archive::run(args),
        Commands::Video(args) => mf_video::run(args),
    }
}
