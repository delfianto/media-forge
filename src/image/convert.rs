use crate::constants::{CHANNEL_BUFFER_MULTIPLIER, MAX_ANALYSIS_SPINNERS};
use crate::image::{ConversionSummary, ImageArgs, ImageError, Result, Task, TaskType};
use crate::walker::{Asset, MediaSource, Walker};
use crate::{CpuControl, IMAGE_EXTENSIONS, Naming, PathUtil, SHUTDOWN, VIDEO_EXTENSIONS, ui};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use image::{DynamicImage, GenericImageView, ImageFormat};
use indicatif::{MultiProgress, ProgressBar};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct WorkItem {
    task: Task,
    pb_container: ProgressBar,
    pb_main: Arc<ProgressBar>,
    complete_signal: Sender<Result<()>>,
}

/// Orchestrates the image conversion process.
pub fn run(args: ImageArgs) -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        println!(
            "\x1b[33mWARNING: Running in DEBUG mode. Performance will be 10-100x slower. Use --release.\x1b[0m"
        );
    }

    let num_threads = CpuControl::get_thread_count(args.jobs);
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global();

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

    let summary = process_tasks(
        tasks_by_container,
        container_names,
        total_files,
        &args,
        num_threads,
    )?;

    // Calculate storage stats
    let original_size = PathUtil::get_dir_size(&source_path);
    let final_size = PathUtil::get_dir_size(&dest_path);

    let mut summary = summary;
    summary.original_size = original_size;
    summary.final_size = final_size;

    summary.print_summary();

    if args.report {
        crate::image::report::generate_conversion_report(&source_path, &dest_path, &args.format)?;
    }

    if summary.exit_code() != 0 {
        return Err(anyhow::anyhow!("Some conversions failed"));
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
    for _ in 0..num_threads.min(MAX_ANALYSIS_SPINNERS) {
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
        let pool = spinner_pool
            .lock()
            .map_err(|_| ImageError::Io(std::io::Error::other("Lock poisoned")))?;
        for pb in pool.iter() {
            pb.finish_and_clear();
        }
    }

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
    let pb_opt = spinner_pool.lock().ok().and_then(|mut pool| pool.pop());

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

    if let Some(pb) = pb_opt
        && let Ok(mut pool) = spinner_pool.lock()
    {
        pool.push(pb);
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
                let fname = path.file_name().unwrap_or_default();
                dest_path.join(fname).with_extension(format)
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
                let fname = path.file_name().unwrap_or_default();
                dest_path.join(fname)
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

/// Executes conversion tasks with progress tracking and result aggregation.
fn process_tasks(
    tasks_by_container: HashMap<String, Vec<Task>>,
    container_names: Vec<String>,
    total_files: usize,
    args: &ImageArgs,
    num_threads: usize,
) -> Result<ConversionSummary> {
    let mp = MultiProgress::new();

    // 1. Main Progress Bar (Top)
    let pb_main = mp.add(ProgressBar::new(total_files as u64));
    pb_main.set_style(ui::main_bar_style());
    pb_main.set_message("Total Progress");

    // 2. Container Progress Bar (Middle)
    let pb_container = mp.add(ProgressBar::new(0));
    pb_container.set_style(ui::sub_bar_style());

    // 3. Worker Spinners (Bottom)
    let mut spinners = Vec::new();
    for _ in 0..num_threads.min(MAX_ANALYSIS_SPINNERS) {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.set_style(ui::generic_spinner_style());
        pb.set_message("Idle");
        spinners.push(pb);
    }
    let spinner_pool = Arc::new(Mutex::new(spinners));

    let start_instant = Instant::now();
    let args_arc = Arc::new(args.clone());
    let pb_main = Arc::new(pb_main);
    // pb_container does not need Arc as it is passed by reference to run_producer (which runs in scoped thread)?
    // Wait, run_producer is called in the scope.
    // pb_container needs to be cloned into WorkItem?
    // WorkItem has pb_container: ProgressBar (which is cheap to clone, it's an Arc internally).

    let (tx, rx) = bounded::<WorkItem>(num_threads * CHANNEL_BUFFER_MULTIPLIER);
    let (results_tx, results_rx) = unbounded::<(PathBuf, Result<()>)>();

    std::thread::scope(|s| {
        spawn_workers(
            s,
            rx,
            args_arc,
            spinner_pool.clone(),
            mp.clone(),
            num_threads,
        );
        run_producer(
            tasks_by_container,
            container_names,
            tx,
            &pb_container, // Pass the bar we created
            &pb_main,
            results_tx,
        );
    });

    {
        if let Ok(pool) = spinner_pool.lock() {
            for pb in pool.iter() {
                pb.disable_steady_tick();
                pb.set_message("");
                pb.finish_and_clear();
            }
        }
    }

    // Clear container bar too
    pb_container.finish_and_clear();

    pb_main.finish_with_message("Done!");
    let duration = start_instant.elapsed();
    println!("Completed in {:.2?}", duration);

    let mut summary = ConversionSummary {
        total: total_files,
        succeeded: 0,
        skipped: 0,
        failed: Vec::new(),
        original_size: 0,
        final_size: 0,
    };

    while let Ok((path, res)) = results_rx.try_recv() {
        match res {
            Ok(()) => summary.succeeded += 1,
            Err(e) => summary.failed.push((path, e.to_string())),
        }
    }

    Ok(summary)
}

/// Spawns worker threads to process tasks from the receiver channel.
fn spawn_workers<'a>(
    scope: &'a std::thread::Scope<'a, '_>,
    rx: Receiver<WorkItem>,
    args: Arc<ImageArgs>,
    spinner_pool: Arc<Mutex<Vec<ProgressBar>>>,
    mp: MultiProgress,
    num_threads: usize,
) {
    for _ in 0..num_threads {
        let rx = rx.clone();
        let args = args.clone();
        let spinner_pool = spinner_pool.clone();
        let mp = mp.clone();

        scope.spawn(move || {
            while let Ok(item) = rx.recv() {
                if SHUTDOWN.load(Ordering::SeqCst) {
                    return;
                }

                process_work_item(item, &args, &spinner_pool, &mp);
            }
        });
    }
}

/// Processes a single work item, updating progress bars and handling task execution.
fn process_work_item(
    item: WorkItem,
    args: &ImageArgs,
    spinner_pool: &Arc<Mutex<Vec<ProgressBar>>>,
    mp: &MultiProgress,
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

    let pb_opt = spinner_pool.lock().ok().and_then(|mut pool| pool.pop());

    if let Some(spinner) = &pb_opt {
        spinner.set_message(format!("Processing: {}", name_display));
        spinner.enable_steady_tick(ui::SPINNER_TICK);
    }

    let result = process_single_task(&item, args);
    if let Err(ref e) = result {
        mp.suspend(|| {
            eprintln!("Error processing {}: {}", name_display, e);
        });
    }

    if let Some(spinner) = pb_opt
        && let Ok(mut pool) = spinner_pool.lock()
    {
        spinner.set_message("Idle");
        spinner.disable_steady_tick();
        pool.push(spinner);
    }

    item.pb_container.inc(1);
    item.pb_main.inc(1);

    let _ = item.complete_signal.send(result);
}

/// Orchestrates task distribution and handles container-level progress bars.
fn run_producer(
    tasks_by_container: HashMap<String, Vec<Task>>,
    container_names: Vec<String>,
    tx: Sender<WorkItem>,
    pb_container: &ProgressBar, // Changed from mp: &MultiProgress
    pb_main: &Arc<ProgressBar>,
    results_tx: Sender<(PathBuf, Result<()>)>,
) {
    // We reuse the passed pb_container

    for container_name in container_names {
        if let Some(tasks) = tasks_by_container.get(&container_name) {
            // Reset and update the existing bar
            pb_container.reset();
            pb_container.set_length(tasks.len() as u64);
            pb_container.set_message(container_name.clone());

            let (done_tx, done_rx) = unbounded::<Result<()>>();

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

            for task in tasks {
                if let Ok(res) = done_rx.recv() {
                    let _ = results_tx.send((task.src_path.clone(), res));
                } else {
                    break;
                }
            }
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
    let rgba_pixels: Vec<ravif::RGBA8> = pixels
        .chunks_exact(4)
        .map(|chunk| ravif::RGBA8 {
            r: chunk[0],
            g: chunk[1],
            b: chunk[2],
            a: chunk[3],
        })
        .collect();

    let img_ravif = ravif::Img::new(rgba_pixels.as_slice(), width as usize, height as usize);
    let out = enc
        .encode_rgba(img_ravif)
        .map_err(|e| ImageError::AvifEncoding(e.to_string()))?;

    fs::write(dest, out.avif_file)?;
    Ok(())
}
