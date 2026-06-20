pub mod archive;
pub mod convert;
pub mod quality;
pub mod report;

use clap::Args as ClapArgs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Helper to load images with AVIF fallback.
pub fn load_image(path: &Path) -> anyhow::Result<image::DynamicImage> {
    match image::open(path) {
        Ok(img) => Ok(img),
        Err(e) => {
            if let Some(ext) = path.extension().and_then(|s| s.to_str())
                && ext.eq_ignore_ascii_case("avif")
            {
                let file = std::fs::File::open(path)?;
                let reader = std::io::BufReader::new(file);
                return image::codecs::avif::AvifDecoder::new(reader)
                    .and_then(image::DynamicImage::from_decoder)
                    .map_err(|e| anyhow::anyhow!("Failed to decode AVIF explicitly: {}", e));
            }
            Err(anyhow::anyhow!("Failed to open image {:?}: {}", path, e))
        }
    }
}

/// Holds aggregated results of a batch conversion process.
pub struct ConversionSummary {
    /// Total number of assets processed.
    pub total: usize,
    /// Number of successfully converted files.
    pub succeeded: usize,
    /// Number of files skipped (e.g., already exists).
    pub skipped: usize,
    /// List of paths and their associated error messages for failed conversions.
    pub failed: Vec<(PathBuf, String)>,
    /// Total size of source directory in bytes.
    pub original_size: u64,
    /// Total size of destination directory in bytes.
    pub final_size: u64,
}

impl ConversionSummary {
    /// Prints a formatted summary of the conversion results to the console.
    pub fn print_summary(&self) {
        println!("\n{}", "=".repeat(50));
        println!("Conversion Summary:");
        println!("  Total Assets: {}", self.total);
        println!("  ✓ Succeeded:  {}", self.succeeded);
        println!("  → Skipped:    {}", self.skipped);

        if !self.failed.is_empty() {
            println!("  ✗ Failed:     {}", self.failed.len());
            for (path, error) in &self.failed {
                println!("    - {:?}: {}", path, error);
            }
        }

        let saved = self.original_size.saturating_sub(self.final_size);
        let saved_percent = if self.original_size > 0 {
            (saved as f64 / self.original_size as f64) * 100.0
        } else {
            0.0
        };

        println!("\nStorage Savings:");
        println!(
            "  Original Size: {}",
            crate::format_size(self.original_size)
        );
        println!("  Final Size:    {}", crate::format_size(self.final_size));
        println!(
            "  Saved:         {} ({:.2}%)",
            crate::format_size(saved),
            saved_percent
        );

        println!("{}\n", "=".repeat(50));
    }

    /// Returns a non-zero exit code if any conversions failed.
    pub fn exit_code(&self) -> i32 {
        if self.failed.is_empty() { 0 } else { 1 }
    }
}

/// Holds aggregated results of an archive creation process.
pub struct ArchiveSummary {
    /// Total number of folders identified for archiving.
    pub total: usize,
    /// Number of successfully created archives.
    pub succeeded: usize,
    /// List of source directory paths and error messages for failed archivals.
    pub failed: Vec<(PathBuf, String)>,
    /// Total size of source directories in bytes.
    pub original_size: u64,
    /// Total size of generated archives in bytes.
    pub final_size: u64,
}

impl ArchiveSummary {
    /// Prints a formatted summary of the archival results to the console.
    pub fn print_summary(&self) {
        println!("\n{}", "=".repeat(50));
        println!("Archival Summary:");
        println!("  Total Folders: {}", self.total);
        println!("  ✓ Succeeded:   {}", self.succeeded);

        if !self.failed.is_empty() {
            println!("  ✗ Failed:      {}", self.failed.len());
            for (path, error) in &self.failed {
                println!("    - {:?}: {}", path, error);
            }
        }

        let saved = self.original_size.saturating_sub(self.final_size);
        let saved_percent = if self.original_size > 0 {
            (saved as f64 / self.original_size as f64) * 100.0
        } else {
            0.0
        };

        println!("\nStorage Savings:");
        println!(
            "  Original Size: {}",
            crate::format_size(self.original_size)
        );
        println!("  Final Size:    {}", crate::format_size(self.final_size));
        println!(
            "  Saved:         {} ({:.2}%)",
            crate::format_size(saved),
            saved_percent
        );

        println!("{}\n", "=".repeat(50));
    }

    /// Returns a non-zero exit code if any archivals failed.
    pub fn exit_code(&self) -> i32 {
        if self.failed.is_empty() { 0 } else { 1 }
    }
}

/// Unified error type for all image and archive operations.
#[derive(Error, Debug)]
pub enum ImageError {
    #[error("Source path does not exist: {0:?}")]
    SourceNotFound(PathBuf),

    #[error("Invalid filename: path {0:?} has no filename component")]
    InvalidFilename(PathBuf),

    #[error("Invalid filename: path {0:?} contains non-UTF8 characters")]
    NonUtf8Filename(PathBuf),

    #[error("Failed to build thread pool: {0}")]
    ThreadPoolError(#[from] rayon::ThreadPoolBuildError),

    #[error("Failed to canonicalize path {0:?}: {1}")]
    CanonicalizationError(PathBuf, #[source] std::io::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("AVIF encoding failed: {0}")]
    AvifEncoding(String),

    #[error("Template error: {0}")]
    Template(#[from] indicatif::style::TemplateError),

    #[error("Could not determine current directory")]
    NoCurrentDir,

    #[error("No image files found in {0:?}")]
    NoImagesFound(PathBuf),

    #[error("Archive verification failed for {0:?}. File count mismatch.")]
    VerificationFailed(PathBuf),

    #[error("Image dimensions mismatch: original {0:?} vs distorted {1:?}")]
    DimensionMismatch((u32, u32), (u32, u32)),
}

/// Convenience result type using ImageError.
pub type Result<T> = std::result::Result<T, ImageError>;

use crate::constants::{DEFAULT_AVIF_SPEED, DEFAULT_IMAGE_QUALITY, DEFAULT_RECURSION_DEPTH};

/// Command-line arguments for image quality analysis (SSIMULACRA2).
#[derive(ClapArgs, Debug, Clone)]
pub struct QualityArgs {
    /// Original reference image or archive (CBZ)
    #[arg(value_name = "ORIGINAL")]
    pub original: PathBuf,

    /// Distorted/encoded image or directory
    #[arg(value_name = "DISTORTED")]
    pub distorted: PathBuf,

    /// Target image format to look for in destination directory
    #[arg(short, long, default_value = "avif", value_name = "FMT")]
    pub format: String,
}

/// Command-line arguments for image conversion.
#[derive(ClapArgs, Debug, Clone)]
pub struct ImageArgs {
    /// Output directory for converted images
    #[arg(value_name = "DEST")]
    pub destination: PathBuf,

    /// Source directories or image files to convert
    #[arg(short, long, default_value = ".", value_name = "DIR", num_args = 1..)]
    pub source: Vec<PathBuf>,

    /// Output image format
    #[arg(short, long, default_value = "avif", value_parser = ["avif", "webp"], value_name = "FMT")]
    pub format: String,

    /// Compression quality level (0-100)
    #[arg(short, long, default_value_t = DEFAULT_IMAGE_QUALITY, value_name = "0-100")]
    pub quality: u8,

    /// AVIF encoding speed (0-10)
    #[arg(long, default_value_t = DEFAULT_AVIF_SPEED, value_name = "0-10")]
    pub speed: u8,

    /// Maximum directory recursion depth
    #[arg(long, default_value_t = DEFAULT_RECURSION_DEPTH, value_name = "N")]
    pub depth: usize,

    /// Number of parallel processing threads
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Disable preservation of original modification times
    #[arg(long)]
    pub no_mtime: bool,

    /// Overwrite existing files even if they are not empty
    #[arg(short, long, alias = "override")]
    pub overwrite: bool,

    /// Enable post-conversion quality report generation
    #[arg(short, long)]
    pub report: bool,
}

/// Command-line arguments for archive creation.
#[derive(ClapArgs, Debug, Clone)]
pub struct ArchiveArgs {
    /// Output directory for CBZ archives
    #[arg(value_name = "DEST")]
    pub destination: Option<PathBuf>,

    /// Source directory (or directories) to scan for image folders
    #[arg(short, long, default_value = ".", value_name = "DIR", num_args = 1..)]
    pub source: Vec<PathBuf>,

    /// Number of parallel processing threads
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Recursively scan for image folders in subdirectories
    #[arg(long, short = 'r')]
    pub recursive: bool,

    /// Delete source folders after successful archiving
    #[arg(long)]
    pub cleanup: bool,

    /// Preview operations without making changes
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Force cleanup without confirmation prompt
    #[arg(long)]
    pub force: bool,
}

/// Discriminates between direct file tasks and internal archive tasks.
#[derive(Clone, Debug)]
pub(crate) enum TaskType {
    /// Processing a standalone image file.
    File,
    /// Processing an image file located within a ZIP/CBZ archive.
    Archive {
        /// The path of the file inside the archive.
        internal_path: String,
    },
    /// Directly copying a non-image file.
    Copy,
}

/// Represents a single image conversion job.
#[derive(Clone, Debug)]
pub(crate) struct Task {
    /// Path to the source file (or archive).
    pub(crate) src_path: PathBuf,
    /// Target path for the converted image.
    pub(crate) dest_path: PathBuf,
    /// Specification of the task origin.
    pub(crate) task_type: TaskType,
}

/// Orchestrates the image conversion process.
pub fn run(args: ImageArgs) -> anyhow::Result<()> {
    convert::run(args)
}

/// Orchestrates the archive creation process.
pub fn run_archive(args: ArchiveArgs) -> anyhow::Result<()> {
    archive::run(args)
}

/// Orchestrates the image quality analysis process.
pub fn run_quality(args: QualityArgs) -> anyhow::Result<()> {
    report::generate_conversion_report(&args.original, &args.distorted, &args.format)
}
