use crate::image::{ImageArgs, ImageError, Result, Task, TaskType};
use crate::walker::{Asset, MediaSource, Walker};
use crate::{
    CpuControl, IMAGE_EXTENSIONS, Naming, PathUtil, SHUTDOWN, VIDEO_EXTENSIONS,
    ui,
};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use image::{DynamicImage, GenericImageView, ImageFormat};
use indicatif::{MultiProgress, ProgressBar};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct WorkItem {
    task: Task,
    pb_container: ProgressBar,
    pb_main: Arc<ProgressBar>,
    complete_signal: Sender<()>,
}

/// Orchestrates the image conversion process.
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

    let mut extensions = Vec::new();
    extensions.extend_from_slice(IMAGE_EXTENSIONS);
    extensions.extend_from_slice(VIDEO_EXTENSIONS);

    let walker = Walker::new(&extensions, args.depth, true);
    let assets = if source_path.is_file() {
        walker.scan_flat(&source_path)
    } else {
        println!("Scanning {}...", source_path.display());
        walker.scan_with_progress(&source_path, "Scanning...")
    };

    let (tasks_by_container, total_files) =
        collect_tasks(assets, &source_path, &dest_path, &args.format, num_threads)?;
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
fn collect_tasks(
    assets: Vec<Asset>,
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

    let pb_total = mp.add(ProgressBar::new(assets.len() as u64));
    pb_total.set_style(ui::main_bar_style());

    let total_files_counter = std::sync::atomic::AtomicUsize::new(0);

    use rayon::prelude::*;
    let all_tasks: Vec<Task> = assets
        .par_iter()
        .map(|asset| {
            process_asset_for_tasks(
                asset,
                source_path,
                dest_path,
                format,
                &spinner_pool,
                &total_files_counter,
                &pb_total,
            )
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
        let container_name = get_container_name(&task, source_path);
        tasks_by_container
            .entry(container_name)
            .or_default()
            .push(task);
    }

    let total_files = total_files_counter.load(Ordering::SeqCst);
    Ok((tasks_by_container, total_files))
}

/// Helper to determine the grouping container name for a task.
fn get_container_name(task: &Task, source_path: &Path) -> String {
    match &task.task_type {
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
    }
}

/// Processes a single scanned asset to generate one or more conversion tasks.
fn process_asset_for_tasks(
    asset: &Asset,
    source_path: &Path,
    dest_path: &Path,
    format: &str,
    spinner_pool: &Arc<Mutex<Vec<ProgressBar>>>,
    total_files_counter: &std::sync::atomic::AtomicUsize,
    pb_total: &ProgressBar,
) -> Vec<Task> {
    let pb_opt = {
        let mut pool = spinner_pool.lock().unwrap();
        pool.pop()
    };

    if let Some(ref pb) = pb_opt {
        let name = asset.path.file_name().unwrap_or_default().to_string_lossy();
        pb.set_message(name.to_string());
        pb.enable_steady_tick(ui::SPINNER_TICK);
    }

    let mut generated_tasks = Vec::new();

    match &asset.source {
        MediaSource::Filesystem(path) => {
            generate_filesystem_tasks(path, source_path, dest_path, format, &mut generated_tasks);
        }
        MediaSource::Archive {
            archive_path,
            entry_name,
        } => {
            generate_archive_tasks(
                archive_path,
                entry_name,
                source_path,
                dest_path,
                format,
                &mut generated_tasks,
            );
        }
    }

    if let Some(ref pb) = pb_opt {
        pb.set_message("Idle");
        pb.disable_steady_tick();
    }

    if let Some(pb) = pb_opt {
        spinner_pool.lock().unwrap().push(pb);
    }

    total_files_counter.fetch_add(generated_tasks.len(), Ordering::SeqCst);
    pb_total.inc(1);
    generated_tasks
}

/// Generates tasks for a file on the filesystem.
fn generate_filesystem_tasks(
    path: &Path,
    source_path: &Path,
    dest_path: &Path,
    format: &str,
    tasks: &mut Vec<Task>,
) {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        let target_file = if source_path.is_file() {
            if dest_path.extension().is_some() {
                dest_path.to_path_buf()
            } else {
                dest_path
                    .join(path.file_name().unwrap())
                    .with_extension(format)
            }
        } else {
            let rel_path = path.strip_prefix(source_path).unwrap_or(path);
            dest_path.join(rel_path).with_extension(format)
        };

        tasks.push(Task {
            src_path: path.to_path_buf(),
            dest_path: target_file,
            task_type: TaskType::File,
        });
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        let target_file = if source_path.is_file() {
            if dest_path.extension().is_some() {
                dest_path.to_path_buf()
            } else {
                dest_path.join(path.file_name().unwrap())
            }
        } else {
            let rel_path = path.strip_prefix(source_path).unwrap_or(path);
            dest_path.join(rel_path)
        };

        tasks.push(Task {
            src_path: path.to_path_buf(),
            dest_path: target_file,
            task_type: TaskType::Copy,
        });
    }
}

/// Generates tasks for an entry inside an archive.
fn generate_archive_tasks(
    archive_path: &Path,
    entry_name: &str,
    source_path: &Path,
    dest_path: &Path,
    format: &str,
    tasks: &mut Vec<Task>,
) {
    let rel_path = archive_path
        .strip_prefix(source_path)
        .unwrap_or(archive_path);
    let parent = rel_path.parent().unwrap_or(Path::new(""));
    let stem = archive_path.file_stem().unwrap_or_default();

    let entry_path = Path::new(entry_name);
    let dest_file = dest_path
        .join(parent)
        .join(stem)
        .join(entry_path)
        .with_extension(format);

    tasks.push(Task {
        src_path: archive_path.to_path_buf(),
        dest_path: dest_file,
        task_type: TaskType::Archive {
            internal_path: entry_name.to_string(),
        },
    });
}

/// Executes conversion tasks with progress tracking.
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
    let args_arc = Arc::new(args.clone());
    let pb_main = Arc::new(pb_main);

    // Channel for distributing work (bounded to limit memory usage)
    let (tx, rx) = bounded::<WorkItem>(num_threads * 2);

    std::thread::scope(|s| {
        spawn_workers(s, rx, args_arc, spinner_pool.clone(), num_threads);
        run_producer(tasks_by_container, container_names, tx, &mp, &pb_main);
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

fn spawn_workers<'a>(
    scope: &'a std::thread::Scope<'a, '_>,
    rx: Receiver<WorkItem>,
    args: Arc<ImageArgs>,
    spinner_pool: Arc<Mutex<Vec<ProgressBar>>>,
    num_threads: usize,
) {
    for _ in 0..num_threads {
        let rx = rx.clone();
        let args = args.clone();
        let spinner_pool = spinner_pool.clone();

        scope.spawn(move || {
            while let Ok(item) = rx.recv() {
                if SHUTDOWN.load(Ordering::SeqCst) {
                    return;
                }

                process_work_item(item, &args, &spinner_pool);
            }
        });
    }
}

fn process_work_item(
    item: WorkItem,
    args: &ImageArgs,
    spinner_pool: &Arc<Mutex<Vec<ProgressBar>>>,
) {
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

    let pb_opt = {
        let mut pool = spinner_pool.lock().unwrap();
        pool.pop()
    };

    if let Some(spinner) = &pb_opt {
        spinner.set_message(format!("Processing: {}", name_display));
        spinner.enable_steady_tick(ui::SPINNER_TICK);
    }

    if let Err(e) = process_single_task(&item, args) {
        eprintln!("Error processing {}: {}", name_display, e);
    }

    if let Some(spinner) = pb_opt {
        spinner.set_message("Idle");
        spinner.disable_steady_tick();
        spinner_pool.lock().unwrap().push(spinner);
    }

    item.pb_container.inc(1);
    item.pb_main.inc(1);

    let _ = item.complete_signal.send(());
}

fn run_producer(
    tasks_by_container: HashMap<String, Vec<Task>>,
    container_names: Vec<String>,
    tx: Sender<WorkItem>,
    mp: &MultiProgress,
    pb_main: &Arc<ProgressBar>,
) {
    for container_name in container_names {
        if let Some(tasks) = tasks_by_container.get(&container_name) {
            let pb_container = mp.add(ProgressBar::new(tasks.len() as u64));
            pb_container.set_style(ui::sub_bar_style());
            pb_container.set_message(container_name.clone());

            let (done_tx, done_rx) = unbounded();

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
                    break;
                }
            }

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
    drop(tx);
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
            drop(img_data);
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
