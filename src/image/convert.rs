use crate::image::{ImageArgs, ImageError, Result, Task, TaskType};
use crate::{
    ARCHIVE_EXTENSIONS, CpuControl, IMAGE_EXTENSIONS, Naming, PathUtil, SHUTDOWN, Scanner,
    VIDEO_EXTENSIONS, ui,
};
use crossbeam_channel::{Sender, bounded};
use image::{DynamicImage, GenericImageView, ImageFormat};
use indicatif::{MultiProgress, ProgressBar};
use rayon::iter::ParallelIterator;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Represents a unit of work to be processed by a worker thread.
struct WorkItem {
    /// The specific task to be executed.
    task: Task,
    /// The progress bar for the container this task belongs to.
    pb_container: ProgressBar,
    /// The main progress bar for the overall process.
    pb_main: Arc<ProgressBar>,
    /// Channel to signal when the task is complete.
    complete_signal: Sender<()>,
}

/// Orchestrates the image conversion process.
///
/// This function initializes the thread pool, resolves paths, scans for files,
/// and manages the concurrent processing of tasks using a bounded channel.
pub fn run(args: ImageArgs) -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        println!(
            "\x1b[33mWARNING: Running in DEBUG mode. Performance will be 10-100x slower. Use --release.\x1b[0m"
        );
    }

    let num_threads = CpuControl::get_thread_count(args.jobs);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .map_err(ImageError::from)?;

    println!("Running with {} threads", num_threads);

    let (source_path, dest_path) = PathUtil::resolve_paths(&args.source, &args.destination)
        .map_err(|_| ImageError::SourceNotFound(args.source.clone()))?;

    let scanner = Scanner::new(args.depth);
    let files = if source_path.is_file() {
        vec![source_path.clone()]
    } else {
        println!("Scanning {}...", source_path.display());
        scanner.scan_with_progress(&source_path, "Scanning...")
    };

    let (tasks_by_container, total_files) =
        collect_tasks(files, &source_path, &dest_path, &args.format, num_threads)?;
    if tasks_by_container.is_empty() {
        println!("No files found to process.");
        return Ok(());
    }

    let mut container_names: Vec<String> = tasks_by_container.keys().cloned().collect();
    container_names.sort();

    println!(
        "Found {} containers with {} files in total.",
        container_names.len(),
        total_files
    );

    process_tasks(
        tasks_by_container,
        container_names,
        total_files,
        &args,
        num_threads,
    )?;

    if args.report {
        crate::image::report::generate_conversion_report(&source_path, &dest_path, &args.format)?;
    }

    Ok(())
}

/// Collects conversion tasks by scanning files and archives in parallel.
///
/// Returns a map of tasks grouped by their parent container (folder or archive)
/// and the total count of files to process.
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
        let pb = mp.insert(0, ProgressBar::new_spinner());
        pb.set_style(ui::analyzing_style());
        pb.set_message("Idle");
        analysis_spinners.push(pb);
    }
    let spinner_pool = Arc::new(Mutex::new(analysis_spinners));

    let pb_total = mp.add(ProgressBar::new(files.len() as u64));
    pb_total.set_style(ui::main_bar_style());

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
                    pb.enable_steady_tick(ui::SPINNER_TICK);
                }

                let t =
                    collect_archive_tasks(file, source_path, dest_path, format).unwrap_or_default();

                if let Some(ref pb) = pb_opt {
                    pb.set_message("Idle");
                    pb.disable_steady_tick();
                }
                t
            } else if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                let target_file = if source_path.is_file() {
                    if dest_path.extension().is_some() {
                        dest_path.to_path_buf()
                    } else {
                        dest_path
                            .join(file.file_name().unwrap())
                            .with_extension(format)
                    }
                } else {
                    let rel_path = file.strip_prefix(source_path).unwrap_or(file);
                    dest_path.join(rel_path).with_extension(format)
                };

                vec![Task {
                    src_path: file.clone(),
                    dest_path: target_file,
                    task_type: TaskType::File,
                }]
            } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
                let target_file = if source_path.is_file() {
                    if dest_path.extension().is_some() {
                        dest_path.to_path_buf()
                    } else {
                        dest_path.join(file.file_name().unwrap())
                    }
                } else {
                    let rel_path = file.strip_prefix(source_path).unwrap_or(file);
                    dest_path.join(rel_path)
                };

                vec![Task {
                    src_path: file.clone(),
                    dest_path: target_file,
                    task_type: TaskType::Copy,
                }]
            } else {
                Vec::new()
            };

            if let Some(pb) = pb_opt {
                spinner_pool.lock().unwrap().push(pb);
            }

            total_files_counter.fetch_add(tasks.len(), Ordering::SeqCst);
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
            TaskType::File | TaskType::Copy => task
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

    let total_files = total_files_counter.load(Ordering::SeqCst);
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

/// Executes conversion tasks with progress tracking using a bounded channel.
///
/// This function sets up a producer-consumer model where tasks are fed into a
/// bounded channel and processed by a fixed pool of worker threads.
fn process_tasks(
    tasks_by_container: HashMap<String, Vec<Task>>,
    container_names: Vec<String>,
    total_files: usize,
    args: &ImageArgs,
    num_threads: usize,
) -> Result<()> {
    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(total_files as u64));
    pb_main.set_style(ui::main_bar_style());
    pb_main.set_message("Total Progress");

    // Worker spinners
    let mut spinners = Vec::new();
    for _ in 0..num_threads {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.set_style(ui::generic_spinner_style());
        pb.set_message("Idle");
        spinners.push(pb);
    }
    let spinner_pool = Arc::new(Mutex::new(spinners));

    let start_instant = Instant::now();
    // Use &args directly or clone if needed for threads. Since threads are scoped, reference is fine?
    // wait, args is passed to threads. threads move. So we need to clone or Arc it?
    // args is ImageArgs struct (Clone).
    // process_tasks takes &args.
    // We can clone it for the loop.
    let args_arc = Arc::new(args.clone());
    let pb_main = Arc::new(pb_main);

    // Channel for distributing work (bounded to limit memory usage)
    let (tx, rx) = bounded::<WorkItem>(num_threads * 2);

    std::thread::scope(|s| {
        // Spawn workers
        for _ in 0..num_threads {
            let rx = rx.clone();
            let args = args_arc.clone();
            let spinner_pool = spinner_pool.clone();

            s.spawn(move || {
                while let Ok(item) = rx.recv() {
                    if SHUTDOWN.load(Ordering::SeqCst) {
                        return;
                    }

                    let name = match &item.task.task_type {
                        TaskType::File | TaskType::Copy => item
                            .task
                            .src_path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        TaskType::Archive { internal_path } => internal_path.clone(),
                    };

                    let name_display = Naming::truncate_from_start(&name, 30);

                    // Acquire spinner for UI feedback
                    let pb_opt = {
                        let mut pool = spinner_pool.lock().unwrap();
                        pool.pop()
                    };

                    if let Some(spinner) = &pb_opt {
                        spinner.set_message(format!("Processing: {}", name_display));
                        spinner.enable_steady_tick(ui::SPINNER_TICK);
                    }

                    if let Err(e) = process_single_task(&item, &args) {
                        eprintln!("Error processing {}: {}", name_display, e);
                    }

                    if let Some(spinner) = pb_opt {
                        spinner.set_message("Idle");
                        spinner.disable_steady_tick();
                        spinner_pool.lock().unwrap().push(spinner);
                    }

                    item.pb_container.inc(1);
                    item.pb_main.inc(1);

                    // Signal completion to the producer
                    let _ = item.complete_signal.send(());
                }
            });
        }

        // Producer loop
        for container_name in container_names {
            if let Some(tasks) = tasks_by_container.get(&container_name) {
                let pb_container = mp.add(ProgressBar::new(tasks.len() as u64));
                pb_container.set_style(ui::sub_bar_style());
                pb_container.set_message(container_name.clone());

                let (done_tx, done_rx) = bounded(0);

                for task in tasks {
                    if SHUTDOWN.load(Ordering::SeqCst) {
                        break;
                    }

                    let item = WorkItem {
                        task: task.clone(),
                        pb_container: pb_container.clone(),
                        pb_main: pb_main.clone(),
                        complete_signal: done_tx.clone(),
                    };

                    if tx.send(item).is_err() {
                        break; // Channel closed
                    }
                }

                // Wait for all tasks in this container to finish.
                drop(done_tx);

                for _ in 0..tasks.len() {
                    if done_rx.recv().is_err() {
                        break;
                    }
                }

                pb_container.finish_and_clear();
                mp.remove(&pb_container);
            }
        }

        // Close channels to stop workers
        drop(tx);
    });

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
fn process_single_task(item: &WorkItem, args: &ImageArgs) -> Result<()> {
    let task = &item.task;

    if PathUtil::should_skip(&task.dest_path, args.overwrite) {
        return Ok(());
    }

    if matches!(task.task_type, TaskType::Copy) {
        if let Some(parent) = task.dest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&task.src_path, &task.dest_path)?;

        if !args.no_mtime
            && let Ok(metadata) = fs::metadata(&task.src_path)
            && let Ok(mtime) = metadata.modified()
            && let Ok(file) = fs::File::open(&task.dest_path)
        {
            file.set_modified(mtime).ok();
        }
        return Ok(());
    }

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
        TaskType::Copy => unreachable!("Copy tasks are handled earlier"),
    };

    if let Some(parent) = task.dest_path.parent() {
        fs::create_dir_all(parent)?;
    }

    match args.format.as_str() {
        "avif" => {
            let img = image::load_from_memory(&img_data)?;
            drop(img_data); // Free raw buffer early to reduce memory pressure
            encode_avif(&img, &task.dest_path, args.quality, args.speed)?;
        }
        "webp" => {
            let img = image::load_from_memory(&img_data)?;
            drop(img_data);
            img.save_with_format(&task.dest_path, ImageFormat::WebP)?;
        }
        _ => unreachable!(),
    };

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

/// Encodes an image to AVIF format with hardware-independent encoding.
fn encode_avif(img: &DynamicImage, dest: &Path, quality: u8, speed: u8) -> Result<()> {
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
