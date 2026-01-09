use clap::Args as ClapArgs;
use image::{DynamicImage, GenericImageView, ImageFormat};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mf_core::{ARCHIVE_EXTENSIONS, CpuControl, IMAGE_EXTENSIONS, Scanner};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use thiserror::Error;

/// Image conversion errors with context-specific information.
#[derive(Error, Debug)]
pub enum ImageError {
    #[error("Source path does not exist: {0:?}")]
    SourceNotFound(PathBuf),

    #[error("Invalid filename: path {0:?} has no filename component")]
    InvalidFilename(PathBuf),

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
}

pub type Result<T> = std::result::Result<T, ImageError>;

/// Command-line arguments for image conversion.
///
/// Converts images to modern compression formats (AVIF, WebP) with configurable
/// quality and speed settings. Supports both direct image files and images
/// inside ZIP/CBZ archives.
#[derive(ClapArgs, Debug, Clone)]
pub struct ImageArgs {
    /// Output directory for converted images
    ///
    /// Converted images will be saved here, preserving the original
    /// directory structure relative to the source path.
    #[arg(value_name = "DEST")]
    pub destination: PathBuf,

    /// Source directory containing images to convert
    ///
    /// Scans this directory for supported image formats (JPG, PNG, WebP,
    /// TIFF, BMP) and archives (ZIP, CBZ) containing images.
    #[arg(short, long, default_value = ".", value_name = "DIR")]
    pub source: PathBuf,

    /// Output image format
    ///
    /// AVIF: Better compression, slower encoding, wider HDR support
    /// WebP: Faster encoding, broader browser compatibility
    #[arg(short, long, default_value = "avif", value_parser = ["avif", "webp"], value_name = "FMT")]
    pub format: String,

    /// Compression quality level (0-100)
    ///
    /// Higher values produce better visual quality but larger files.
    /// Recommended: 70-85 for general use, 85-95 for photography.
    /// Default of 72 provides good balance for most content.
    #[arg(short, long, default_value_t = 72, value_name = "0-100")]
    pub quality: u8,

    /// AVIF encoding speed (0-10)
    ///
    /// Lower values produce smaller files but take longer to encode.
    /// 0-2: Best compression, very slow (archival)
    /// 3-5: Good compression, moderate speed (recommended)
    /// 6-10: Fast encoding, larger files (previews)
    /// Only affects AVIF format; ignored for WebP.
    #[arg(long, default_value_t = 4, value_name = "0-10")]
    pub speed: u8,

    /// Maximum directory recursion depth
    ///
    /// How many levels of subdirectories to scan for images.
    /// Use higher values for deeply nested folder structures.
    #[arg(long, default_value_t = 2, value_name = "N")]
    pub depth: usize,

    /// Number of parallel processing threads
    ///
    /// Defaults to 75% of available CPU cores for optimal performance.
    /// Maximum allowed is 150% of cores. Reduce if memory-constrained.
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Disable preservation of original modification times
    ///
    /// By default, converted images retain the modification time of
    /// their source files. Use this flag to set current time instead.
    #[arg(long)]
    pub no_mtime: bool,
}

#[derive(Clone, Debug)]
enum TaskType {
    File,
    Archive { internal_path: String },
}

#[derive(Clone, Debug)]
struct Task {
    src_path: PathBuf,
    dest_path: PathBuf,
    task_type: TaskType,
}

/// Orchestrates the image conversion process.
pub fn run(args: ImageArgs) -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        println!(
            "\x1b[33mWARNING: Running in DEBUG mode. Performance will be 10-100x slower. Use --release.\x1b[0m"
        );
    }

    // Configure thread pool
    let num_threads = CpuControl::get_thread_count(args.jobs);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .map_err(ImageError::from)?;

    println!("Running with {} threads", num_threads);

    // Resolve paths
    let source_path = fs::canonicalize(&args.source)
        .map_err(|e| ImageError::CanonicalizationError(args.source.clone(), e))?;

    if !source_path.exists() {
        return Err(ImageError::SourceNotFound(source_path).into());
    }

    let dest_path = if args.destination.is_absolute() {
        args.destination.clone()
    } else {
        std::env::current_dir()
            .map_err(|_| ImageError::NoCurrentDir)?
            .join(&args.destination)
    };

    // Collect tasks
    println!("Scanning '{:?}'...", source_path);
    let scanner = Scanner::new(args.depth);
    let files = scanner.scan(&source_path);

    let (tasks_by_container, total_files) =
        collect_tasks(files, &source_path, &dest_path, &args.format)?;

    if tasks_by_container.is_empty() {
        println!("No images or archives found to process.");
        return Ok(());
    }

    let mut container_names: Vec<String> = tasks_by_container.keys().cloned().collect();
    container_names.sort();

    println!(
        "Found {} containers with {} files in total.",
        container_names.len(),
        total_files
    );

    // Execute tasks
    process_tasks(
        tasks_by_container,
        container_names,
        total_files,
        args,
        num_threads,
    )?;

    Ok(())
}

/// Collects conversion tasks by scanning files and archives.
fn collect_tasks(
    files: Vec<PathBuf>,
    source_path: &Path,
    dest_path: &Path,
    format: &str,
) -> Result<(HashMap<String, Vec<Task>>, usize)> {
    let mut tasks_by_container: HashMap<String, Vec<Task>> = HashMap::new();
    let mut total_files = 0;

    for file in files {
        let ext = file
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        if ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
            let count = collect_archive_tasks(
                &file,
                source_path,
                dest_path,
                format,
                &mut tasks_by_container,
            )?;
            total_files += count;
        } else if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            let rel_path = file.strip_prefix(source_path).unwrap_or(&file);
            let target_file = dest_path.join(rel_path).with_extension(format);

            let parent_folder = rel_path
                .parent()
                .unwrap_or(Path::new("root"))
                .to_string_lossy()
                .to_string();

            tasks_by_container
                .entry(parent_folder)
                .or_default()
                .push(Task {
                    src_path: file,
                    dest_path: target_file,
                    task_type: TaskType::File,
                });
            total_files += 1;
        }
    }

    Ok((tasks_by_container, total_files))
}

/// Scans a ZIP archive for images and creates corresponding tasks.
fn collect_archive_tasks(
    archive_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    format: &str,
    tasks_by_container: &mut HashMap<String, Vec<Task>>,
) -> Result<usize> {
    let mut count = 0;
    let zip_file = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(zip_file)?;

    let rel_path = archive_path
        .strip_prefix(source_path)
        .unwrap_or(archive_path);
    let parent = rel_path.parent().unwrap_or(Path::new(""));
    let stem = archive_path.file_stem().unwrap_or_default();
    let cbz_dest_folder = dest_path.join(parent).join(stem);

    let container_name = archive_path
        .file_name()
        .ok_or_else(|| ImageError::InvalidFilename(archive_path.to_path_buf()))?
        .to_string_lossy()
        .to_string();

    for i in 0..archive.len() {
        let a_file = archive.by_index(i)?;
        if a_file.is_file() {
            let a_ext = Path::new(a_file.name())
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();

            if IMAGE_EXTENSIONS.contains(&a_ext.as_str()) {
                let target_file = cbz_dest_folder.join(a_file.name()).with_extension(format);

                tasks_by_container
                    .entry(container_name.clone())
                    .or_default()
                    .push(Task {
                        src_path: archive_path.to_path_buf(),
                        dest_path: target_file,
                        task_type: TaskType::Archive {
                            internal_path: a_file.name().to_string(),
                        },
                    });
                count += 1;
            }
        }
    }

    Ok(count)
}

/// Executes conversion tasks with progress tracking.
fn process_tasks(
    tasks_by_container: HashMap<String, Vec<Task>>,
    container_names: Vec<String>,
    total_files: usize,
    args: ImageArgs,
    num_threads: usize,
) -> Result<()> {
    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(total_files as u64));
    pb_main.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")?
            .progress_chars("#>-"),
    );
    pb_main.set_message("Total Progress");

    let mut spinners = Vec::new();
    for _ in 0..num_threads {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.blue} {msg}")
                .unwrap(),
        );
        pb.set_message("Idle");
        spinners.push(pb);
    }
    let spinner_pool = Arc::new(Mutex::new(spinners));

    let start_instant = Instant::now();
    let args = Arc::new(args);
    let pb_main = Arc::new(pb_main);

    for container_name in container_names {
        if let Some(tasks) = tasks_by_container.get(&container_name) {
            let pb_container = mp.add(ProgressBar::new(tasks.len() as u64));

            pb_container.set_style(
                ProgressStyle::default_bar()
                    .template("  {bar:20.magenta} {pos}/{len} {msg}")
                    .unwrap()
                    .progress_chars("=>-"),
            );
            pb_container.set_message(container_name.clone());

            tasks.par_iter().for_each(|task| {
                let name = match &task.task_type {
                    TaskType::File => task
                        .src_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    TaskType::Archive { internal_path } => internal_path.clone(),
                };

                let name_display = if name.len() > 30 {
                    format!("...{}", &name[name.len().saturating_sub(27)..])
                } else {
                    name
                };

                let pb_opt = {
                    let mut pool = spinner_pool.lock().unwrap();
                    pool.pop()
                };

                if let Some(spinner) = pb_opt {
                    spinner.set_message(format!("Processing: {}", name_display));
                    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

                    if let Err(e) = process_single_task(task, &args) {
                        mp.suspend(|| eprintln!("Error processing {}: {}", name_display, e));
                    }

                    spinner.set_message("Idle");
                    spinner.disable_steady_tick();

                    spinner_pool.lock().unwrap().push(spinner);
                } else if let Err(e) = process_single_task(task, &args) {
                    mp.suspend(|| eprintln!("Error processing {}: {}", name_display, e));
                }

                pb_main.inc(1);
                pb_container.inc(1);
            });

            pb_container.finish_and_clear();
            mp.remove(&pb_container);
        }
    }

    {
        let pool = spinner_pool.lock().unwrap();
        for pb in pool.iter() {
            pb.finish_and_clear();
            mp.remove(pb);
        }
    }

    pb_main.finish_with_message("Done!");
    let duration = start_instant.elapsed();
    println!("Completed in {:.2?}", duration);

    Ok(())
}

/// Processes a single image conversion task.
fn process_single_task(task: &Task, args: &ImageArgs) -> Result<()> {
    let img_data = match &task.task_type {
        TaskType::File => fs::read(&task.src_path)?,
        TaskType::Archive { internal_path } => {
            let zip_file = fs::File::open(&task.src_path)?;
            let mut archive = zip::ZipArchive::new(zip_file)?;
            let mut a_file = archive.by_name(internal_path)?;
            let mut buffer = Vec::new();
            std::io::Read::read_to_end(&mut a_file, &mut buffer)?;
            buffer
        }
    };

    let img = image::load_from_memory(&img_data)?;

    if let Some(parent) = task.dest_path.parent() {
        fs::create_dir_all(parent)?;
    }

    match args.format.as_str() {
        "avif" => {
            encode_avif(img, &task.dest_path, args.quality, args.speed)?;
        }
        "webp" => {
            img.save_with_format(&task.dest_path, ImageFormat::WebP)?;
        }
        _ => unreachable!(),
    }

    if !args.no_mtime
        && matches!(task.task_type, TaskType::File)
        && let Ok(metadata) = fs::metadata(&task.src_path)
        && let Ok(mtime) = metadata.modified()
    {
        let file = fs::File::open(&task.dest_path)?;
        file.set_modified(mtime).ok();
    }

    Ok(())
}

/// Encodes an image to AVIF format.
fn encode_avif(img: DynamicImage, dest: &Path, quality: u8, speed: u8) -> Result<()> {
    let (width, height) = img.dimensions();
    let img_rgba = img.to_rgba8();

    let enc = ravif::Encoder::new()
        .with_quality(quality as f32)
        .with_speed(speed);

    let pixels = img_rgba.as_raw();
    let rgba_pixels = unsafe {
        std::slice::from_raw_parts(pixels.as_ptr() as *const ravif::RGBA8, pixels.len() / 4)
    };

    let img_ravif = ravif::Img::new(rgba_pixels, width as usize, height as usize);
    let out = enc
        .encode_rgba(img_ravif)
        .map_err(|e| ImageError::AvifEncoding(e.to_string()))?;

    fs::write(dest, out.avif_file)?;
    Ok(())
}
