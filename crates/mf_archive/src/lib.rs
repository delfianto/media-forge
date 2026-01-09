use clap::Args as ClapArgs;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mf_core::{CpuControl, IMAGE_EXTENSIONS};
use rayon::prelude::*;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use zip::write::SimpleFileOptions;

/// Archive creation errors with context-specific information.
#[derive(Error, Debug)]
pub enum ArchiveError {
    #[error("No image files found in {0:?}")]
    NoImagesFound(PathBuf),

    #[error("Archive verification failed for {0:?}. File count mismatch.")]
    VerificationFailed(PathBuf),

    #[error("Invalid filename: path {0:?} has no filename component")]
    InvalidFilename(PathBuf),

    #[error("Invalid filename: path {0:?} contains non-UTF8 characters")]
    NonUtf8Filename(PathBuf),

    #[error("Could not determine current directory")]
    NoCurrentDir,

    #[error("Source path does not exist: {0:?}")]
    SourceNotFound(PathBuf),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Failed to build thread pool: {0}")]
    ThreadPoolError(#[from] rayon::ThreadPoolBuildError),

    #[error("Template error: {0}")]
    Template(#[from] indicatif::style::TemplateError),
}

pub type Result<T> = std::result::Result<T, ArchiveError>;

/// Command-line arguments for archive creation.
#[derive(ClapArgs, Debug, Clone)]
pub struct ArchiveArgs {
    /// Output directory for CBZ archives
    #[arg(value_name = "DEST")]
    pub destination: Option<PathBuf>,

    /// Source directory to scan for image folders
    #[arg(short, long, default_value = ".", value_name = "DIR")]
    pub source: PathBuf,

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

struct ArchiveTask {
    src_dir: PathBuf,
    dest_cbz: PathBuf,
}

/// Main entry point for archive creation.
pub fn run(args: ArchiveArgs) -> anyhow::Result<()> {
    // Configure thread pool
    let num_threads = CpuControl::get_thread_count(args.jobs);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .map_err(ArchiveError::from)?;

    // Resolve paths
    let source_path = fs::canonicalize(&args.source)
        .map_err(|_| ArchiveError::SourceNotFound(args.source.clone()))?;

    let dest_path = args
        .destination
        .as_ref()
        .map(|d| {
            if d.is_absolute() {
                Ok(d.clone())
            } else {
                std::env::current_dir()
                    .map_err(|_| ArchiveError::NoCurrentDir)
                    .map(|cwd| cwd.join(d))
            }
        })
        .transpose()?;

    // Discover image folders to archive
    println!("Scanning {} for image folders...", source_path.display());
    let pb_scan = ProgressBar::new_spinner();
    pb_scan.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg} {pos} items found")
            .unwrap(),
    );
    pb_scan.enable_steady_tick(std::time::Duration::from_millis(100));

    let mut items_found = 0;
    let tasks = collect_archive_tasks(&source_path, &dest_path, args.recursive, |path| {
        items_found += 1;
        pb_scan.set_position(items_found);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        pb_scan.set_message(format!("Scanning: {}", name));
    })?;
    pb_scan.finish_and_clear();

    if tasks.is_empty() {
        println!("No folders found to archive.");
        return Ok(());
    }

    // Handle dry-run mode
    if args.dry_run {
        display_dry_run_preview(&tasks, args.cleanup);
        return Ok(());
    }

    // Safety check: Require confirmation before cleanup unless --force is used
    if args.cleanup && !args.force && !prompt_for_cleanup_confirmation(&tasks)? {
        return Ok(());
    }

    // Execute archive tasks
    execute_archive_tasks(tasks, Arc::new(args), num_threads)?;

    Ok(())
}

fn collect_archive_tasks<F>(
    source_root: &Path,
    dest_root: &Option<PathBuf>,
    recursive: bool,
    mut callback: F,
) -> Result<Vec<ArchiveTask>>
where
    F: FnMut(&Path),
{
    let mut tasks = Vec::new();
    find_image_folders(
        source_root,
        source_root,
        dest_root,
        recursive,
        &mut tasks,
        &mut callback,
    )?;
    Ok(tasks)
}

fn find_image_folders<F>(
    current: &Path,
    source_root: &Path,
    dest_root: &Option<PathBuf>,
    recursive: bool,
    tasks: &mut Vec<ArchiveTask>,
    callback: &mut F,
) -> Result<()>
where
    F: FnMut(&Path),
{
    callback(current);
    let mut has_images = false;
    let mut entries = Vec::new();

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                has_images = true;
            }
        } else if path.is_dir() && recursive {
            entries.push(path);
        }
    }

    if has_images {
        let rel_path = current
            .strip_prefix(source_root)
            .map_err(io::Error::other)?;

        let dest_folder = if let Some(dr) = dest_root {
            dr.join(rel_path.parent().unwrap_or_else(|| Path::new("")))
        } else {
            current
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_path_buf()
        };

        let filename = current
            .file_name()
            .ok_or_else(|| ArchiveError::InvalidFilename(current.to_path_buf()))?;
        let filename_str = filename
            .to_str()
            .ok_or_else(|| ArchiveError::NonUtf8Filename(current.to_path_buf()))?;
        let cbz_name = format!("{}.cbz", filename_str);
        let dest_cbz = dest_folder.join(cbz_name);

        tasks.push(ArchiveTask {
            src_dir: current.to_path_buf(),
            dest_cbz,
        });
    }

    for dir in entries {
        find_image_folders(&dir, source_root, dest_root, recursive, tasks, callback)?;
    }

    Ok(())
}

fn display_dry_run_preview(tasks: &[ArchiveTask], cleanup: bool) {
    println!("-- DRY RUN: {} tasks --", tasks.len());
    for task in tasks {
        let action = if cleanup {
            "ARCHIVE & DELETE"
        } else {
            "ARCHIVE"
        };
        println!("[{}] {:?} -> {:?}", action, task.src_dir, task.dest_cbz);
    }
}

fn prompt_for_cleanup_confirmation(tasks: &[ArchiveTask]) -> anyhow::Result<bool> {
    println!("\n\x1b[33m⚠️  WARNING: CLEANUP MODE ENABLED\x1b[0m");
    println!(
        "The following {} folder(s) will be PERMANENTLY DELETED after archiving:",
        tasks.len()
    );

    for (i, task) in tasks.iter().enumerate().take(10) {
        println!("  {}. {:?}", i + 1, task.src_dir);
    }
    if tasks.len() > 10 {
        println!("  ... and {} more folders", tasks.len() - 10);
    }

    println!("\n\x1b[31mThis action CANNOT be undone!\x1b[0m");
    println!("To preview without changes, use: --dry-run --cleanup");
    println!("To skip this prompt, use: --force (automated scripts only)\n");

    print!("Type 'DELETE' (all caps) to confirm deletion: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input != "DELETE" {
        println!("\n\x1b[32mOperation cancelled. No files were modified.\x1b[0m");
        return Ok(false);
    }

    println!("\n\x1b[33mConfirmed. Proceeding with archiving and cleanup...\x1b[0m\n");
    Ok(true)
}

fn execute_archive_tasks(
    tasks: Vec<ArchiveTask>,
    args: Arc<ArchiveArgs>,
    num_threads: usize,
) -> Result<()> {
    println!(
        "Archiving {} folders with {} threads...",
        tasks.len(),
        num_threads
    );

    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(tasks.len() as u64));
    pb_main.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}"
            )?
            .progress_chars("#>-")
    );

    let pb_main = Arc::new(pb_main);

    tasks.par_iter().for_each(|task| {
        let pb_inner = mp.add(ProgressBar::new_spinner());
        pb_inner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.blue} {msg}")
                .unwrap(),
        );
        pb_inner.set_message(format!(
            "{:?}",
            task.src_dir.file_name().unwrap_or_default()
        ));

        if let Err(e) = create_cbz(task, &args) {
            eprintln!("Error archiving {:?}: {}", task.src_dir, e);
        }

        mp.remove(&pb_inner);
        pb_main.inc(1);
    });

    pb_main.finish_with_message("Done!");
    Ok(())
}

fn create_cbz(task: &ArchiveTask, args: &ArchiveArgs) -> Result<()> {
    if let Some(parent) = task.dest_cbz.parent() {
        fs::create_dir_all(parent)?;
    }

    let files = collect_and_sort_images(&task.src_dir)?;

    let file = fs::File::create(&task.dest_cbz)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    write_files_to_zip(&mut zip, &files, options)?;

    zip.finish()?;

    verify_archive(&task.dest_cbz, files.len())?;

    if args.cleanup && !args.dry_run {
        fs::remove_dir_all(&task.src_dir)?;
    }

    Ok(())
}

fn collect_and_sort_images(src_dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut files: Vec<_> = fs::read_dir(src_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            if p.is_file() {
                let ext = p
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                IMAGE_EXTENSIONS.contains(&ext.as_str()) || ext == "xml"
            } else {
                false
            }
        })
        .collect();

    files.sort_by(|a, b| {
        natord::compare(
            &a.file_name().to_string_lossy(),
            &b.file_name().to_string_lossy(),
        )
    });

    if files.is_empty() {
        return Err(ArchiveError::NoImagesFound(src_dir.to_path_buf()));
    }

    Ok(files)
}

fn write_files_to_zip(
    zip: &mut zip::ZipWriter<fs::File>,
    files: &[fs::DirEntry],
    options: SimpleFileOptions,
) -> Result<()> {
    for entry in files {
        let path = entry.path();
        let arc_name = path
            .file_name()
            .ok_or_else(|| ArchiveError::InvalidFilename(path.clone()))?
            .to_string_lossy();

        zip.start_file(arc_name, options)?;
        let content = fs::read(&path)?;
        zip.write_all(&content)?;
    }

    Ok(())
}

fn verify_archive(archive_path: &Path, expected_count: usize) -> Result<()> {
    let file = fs::File::open(archive_path)?;
    let archive = zip::ZipArchive::new(file)?;

    if archive.len() != expected_count {
        return Err(ArchiveError::VerificationFailed(archive_path.to_path_buf()));
    }

    Ok(())
}
