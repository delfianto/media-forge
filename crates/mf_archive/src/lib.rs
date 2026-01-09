use anyhow::{Result, anyhow};
use clap::Args as ClapArgs;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mf_core::{CpuControl, IMAGE_EXTENSIONS};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zip::write::SimpleFileOptions;

/// Command-line arguments for archive creation.
///
/// Creates CBZ (Comic Book ZIP) archives from folders containing images.
/// Archives are formatted for optimal compatibility with comic readers,
/// with automatic page numbering and natural file sorting.
#[derive(ClapArgs, Debug, Clone)]
pub struct ArchiveArgs {
    /// Output directory for CBZ archives
    ///
    /// If not specified, archives are created alongside source folders.
    /// Directory structure is preserved relative to the source path.
    #[arg(value_name = "DEST")]
    pub destination: Option<PathBuf>,

    /// Source directory to scan for image folders
    ///
    /// Searches for folders containing image files (JPG, PNG, WebP, etc.).
    /// Each qualifying folder becomes a separate CBZ archive.
    #[arg(short, long, default_value = ".", value_name = "DIR")]
    pub source: PathBuf,

    /// Number of parallel processing threads
    ///
    /// Defaults to 75% of available CPU cores. Higher values can
    /// improve throughput but may increase I/O contention.
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Recursively scan for image folders in subdirectories
    ///
    /// Without this flag, only immediate subdirectories are considered.
    /// Enable to process deeply nested folder structures.
    #[arg(long, short = 'r')]
    pub recursive: bool,

    /// Delete source folders after successful archiving
    ///
    /// WARNING: This permanently removes the original image folders
    /// after verifying the archive was created correctly. Use with
    /// --dry-run first to preview which folders will be deleted.
    #[arg(long)]
    pub cleanup: bool,

    /// Preview operations without making changes
    ///
    /// Shows what archives would be created and which folders would
    /// be affected, without actually creating any files or deleting
    /// any folders. Recommended before using --cleanup.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
}

struct ArchiveTask {
    src_dir: PathBuf,
    dest_cbz: PathBuf,
}

pub fn run(args: ArchiveArgs) -> Result<()> {
    let num_threads = CpuControl::get_thread_count(args.jobs);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()?;

    let source_path = fs::canonicalize(&args.source)?;
    let dest_path = args.destination.clone().map(|d| {
        if d.is_absolute() {
            d
        } else {
            std::env::current_dir().unwrap().join(d)
        }
    });

    println!("Scanning '{:?}' for image folders...", source_path);
    let mut tasks = Vec::new();
    find_image_folders(
        &source_path,
        &source_path,
        &dest_path,
        args.recursive,
        &mut tasks,
    )?;

    if tasks.is_empty() {
        println!("No folders found to archive.");
        return Ok(());
    }

    if args.dry_run {
        println!("--- DRY RUN: {} tasks ---", tasks.len());
        for task in &tasks {
            let action = if args.cleanup {
                "ARCHIVE & DELETE"
            } else {
                "ARCHIVE"
            };
            println!("[{}] {:?} -> {:?}", action, task.src_dir, task.dest_cbz);
        }
        return Ok(());
    }

    println!(
        "Archiving {} folders with {} threads...",
        tasks.len(),
        num_threads
    );

    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(tasks.len() as u64));
    pb_main.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")?
            .progress_chars("#>-"),
    );

    let args = Arc::new(args);
    let pb_main = Arc::new(pb_main);

    tasks.par_iter().for_each(|task| {
        if let Err(e) = create_cbz(task, &args) {
            eprintln!("Error archiving {:?}: {}", task.src_dir, e);
        }
        pb_main.inc(1);
    });

    pb_main.finish_with_message("Done!");
    Ok(())
}

fn find_image_folders(
    current: &Path,
    source_root: &Path,
    dest_root: &Option<PathBuf>,
    recursive: bool,
    tasks: &mut Vec<ArchiveTask>,
) -> Result<()> {
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
        let rel_path = current.strip_prefix(source_root)?;
        let dest_folder = if let Some(dr) = dest_root {
            dr.join(rel_path.parent().unwrap_or_else(|| Path::new("")))
        } else {
            current
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_path_buf()
        };

        let cbz_name = format!("{}.cbz", current.file_name().unwrap().to_str().unwrap());
        let dest_cbz = dest_folder.join(cbz_name);

        tasks.push(ArchiveTask {
            src_dir: current.to_path_buf(),
            dest_cbz,
        });
    }

    for dir in entries {
        find_image_folders(&dir, source_root, dest_root, recursive, tasks)?;
    }

    Ok(())
}

fn create_cbz(task: &ArchiveTask, args: &ArchiveArgs) -> Result<()> {
    if let Some(parent) = task.dest_cbz.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = fs::File::create(&task.dest_cbz)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let mut files: Vec<_> = fs::read_dir(&task.src_dir)?
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
        return Err(anyhow!("No valid files found in {:?}", task.src_dir));
    }

    for (i, entry) in files.iter().enumerate() {
        let path = entry.path();
        let _original_name = path.file_name().unwrap().to_str().unwrap();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

        let arc_name = if i == 0 {
            format!("000_cover.{}", ext)
        } else if i == files.len() - 1 {
            format!("999_back.{}", ext)
        } else {
            format!("page_{:03}.{}", i, ext)
        };

        zip.start_file(arc_name, options)?;
        let content = fs::read(&path)?;
        std::io::Write::write_all(&mut zip, &content)?;
    }

    zip.finish()?;

    {
        let file = fs::File::open(&task.dest_cbz)?;
        let archive = zip::ZipArchive::new(file)?;
        if archive.len() != files.len() {
            return Err(anyhow!("Verification failed for {:?}", task.dest_cbz));
        }
    }

    if args.cleanup {
        fs::remove_dir_all(&task.src_dir)?;
    }

    Ok(())
}
