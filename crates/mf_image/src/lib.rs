use clap::Args as ClapArgs;
use image::{DynamicImage, GenericImageView, ImageFormat};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mf_core::{ARCHIVE_EXTENSIONS, CpuControl, IMAGE_EXTENSIONS, SHUTDOWN, Scanner};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
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
#[derive(ClapArgs, Debug, Clone)]
pub struct ImageArgs {
    /// Output directory for converted images
    #[arg(value_name = "DEST")]
    pub destination: PathBuf,

    /// Source directory containing images to convert
    #[arg(short, long, default_value = ".", value_name = "DIR")]
    pub source: PathBuf,

    /// Output image format
    #[arg(short, long, default_value = "avif", value_parser = ["avif", "webp"], value_name = "FMT")]
    pub format: String,

    /// Compression quality level (0-100)
    #[arg(short, long, default_value_t = 72, value_name = "0-100")]
    pub quality: u8,

    /// AVIF encoding speed (0-10)
    #[arg(long, default_value_t = 4, value_name = "0-10")]
    pub speed: u8,

    /// Maximum directory recursion depth
    #[arg(long, default_value_t = 2, value_name = "N")]
    pub depth: usize,

    /// Number of parallel processing threads
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Disable preservation of original modification times
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
    println!("Scanning {}...", source_path.display());
    let pb_scan = ProgressBar::new_spinner();
    pb_scan.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg} {pos} items found")
            .unwrap(),
    );
    pb_scan.enable_steady_tick(std::time::Duration::from_millis(100));

    let scanner = Scanner::new(args.depth);
    let mut files_found = 0;
    let files = scanner.scan_with_callback(&source_path, |path| {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if path.is_file() {
            files_found += 1;
            pb_scan.set_position(files_found);
        }
        pb_scan.set_message(format!("Scanning: {}", name));
    });
    pb_scan.finish_and_clear();

    let (tasks_by_container, total_files) =
        collect_tasks(files, &source_path, &dest_path, &args.format, num_threads)?;

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

/// Collects conversion tasks by scanning files and archives in parallel.
fn collect_tasks(
    files: Vec<PathBuf>,
    source_path: &Path,
    dest_path: &Path,
    format: &str,
    num_threads: usize,
) -> Result<(HashMap<String, Vec<Task>>, usize)> {
    let mp = MultiProgress::new();

    let mut analysis_spinners = Vec::new();
    for _ in 0..num_threads.min(8) {
        // Limit spinners to avoid screen flooding
        let pb = mp.insert(0, ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.blue} Analyzing: {msg}")
                .unwrap(),
        );
        pb.set_message("Idle");
        analysis_spinners.push(pb);
    }
    let spinner_pool = Arc::new(Mutex::new(analysis_spinners));

    let pb_total = mp.add(ProgressBar::new(files.len() as u64));
    pb_total.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} Total Analysis: [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
            .unwrap(),
    );

    let total_files_counter = std::sync::atomic::AtomicUsize::new(0);

    let all_tasks: Vec<Task> = files
        .par_iter()
        .map(|file| {
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();

            let pb_opt = {
                let mut pool = spinner_pool.lock().unwrap();
                pool.pop()
            };

            let ext = file
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();

            let tasks = if ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
                if let Some(ref pb) = pb_opt {
                    pb.set_message(name.to_string());
                    pb.enable_steady_tick(std::time::Duration::from_millis(100));
                }

                let t =
                    collect_archive_tasks(file, source_path, dest_path, format).unwrap_or_default();

                if let Some(ref pb) = pb_opt {
                    pb.set_message("Idle");
                    pb.disable_steady_tick();
                }
                t
            } else if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                let rel_path = file.strip_prefix(source_path).unwrap_or(file);
                let target_file = dest_path.join(rel_path).with_extension(format);

                vec![Task {
                    src_path: file.clone(),
                    dest_path: target_file,
                    task_type: TaskType::File,
                }]
            } else {
                Vec::new()
            };

            if let Some(pb) = pb_opt {
                spinner_pool.lock().unwrap().push(pb);
            }

            total_files_counter.fetch_add(tasks.len(), std::sync::atomic::Ordering::SeqCst);
            pb_total.inc(1);
            tasks
        })
        .flatten()
        .collect();

    {
        let pool = spinner_pool.lock().unwrap();
        for pb in pool.iter() {
            pb.finish_and_clear();
            mp.remove(pb);
        }
    }

    pb_total.finish_and_clear();

    let mut tasks_by_container: HashMap<String, Vec<Task>> = HashMap::new();
    for task in all_tasks {
        let container_name = match &task.task_type {
            TaskType::File => task
                .src_path
                .strip_prefix(source_path)
                .unwrap_or(&task.src_path)
                .parent()
                .unwrap_or(Path::new("root"))
                .to_string_lossy()
                .to_string(),
            TaskType::Archive { .. } => task
                .src_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown_archive".to_string()),
        };

        tasks_by_container
            .entry(container_name)
            .or_default()
            .push(task);
    }

    let total_files = total_files_counter.load(std::sync::atomic::Ordering::SeqCst);
    Ok((tasks_by_container, total_files))
}

/// Scans a ZIP archive for images and creates corresponding tasks.
fn collect_archive_tasks(
    archive_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    format: &str,
) -> Result<Vec<Task>> {
    let mut tasks = Vec::new();

    let zip_file = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(zip_file)?;

    let rel_path = archive_path
        .strip_prefix(source_path)
        .unwrap_or(archive_path);
    let parent = rel_path.parent().unwrap_or(Path::new(""));
    let stem = archive_path.file_stem().unwrap_or_default();
    let cbz_dest_folder = dest_path.join(parent).join(stem);

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

                tasks.push(Task {
                    src_path: archive_path.to_path_buf(),
                    dest_path: target_file,
                    task_type: TaskType::Archive {
                        internal_path: a_file.name().to_string(),
                    },
                });
            }
        }
    }

    Ok(tasks)
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
                if SHUTDOWN.load(Ordering::SeqCst) {
                    return;
                }

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
