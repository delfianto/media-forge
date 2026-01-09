use anyhow::{Result, anyhow};
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

#[derive(ClapArgs, Debug, Clone)]
pub struct ImageArgs {
    /// Destination directory
    pub destination: PathBuf,

    /// Source directory
    #[arg(short, long, default_value = ".")]
    pub source: PathBuf,

    /// Output format
    #[arg(short, long, default_value = "avif", value_parser = ["avif", "webp"])]
    pub format: String,

    /// Compression quality 0-100
    #[arg(short, long, default_value_t = 72)]
    pub quality: u8,

    /// AVIF Speed 0-10 (Lower is slower/smaller)
    #[arg(long, default_value_t = 4)]
    pub speed: u8,

    /// Folder recursion depth
    #[arg(long, default_value_t = 2)]
    pub depth: usize,

    /// Number of threads (Defaults to 75% of cores. Max 1.5x cores)
    #[arg(short, long)]
    pub jobs: Option<usize>,

    /// Do not preserve original file modification times
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

pub fn run(args: ImageArgs) -> Result<()> {
    if cfg!(debug_assertions) {
        println!(
            "\x1b[33mWARNING: Running in DEBUG mode. Performance will be 10-100x slower. Use --release.\x1b[0m"
        );
    }

    let num_threads = CpuControl::get_thread_count(args.jobs);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()?;

    println!("Running with {} threads", num_threads);

    let source_path = fs::canonicalize(&args.source)?;
    let dest_path = if args.destination.is_absolute() {
        args.destination.clone()
    } else {
        std::env::current_dir()?.join(&args.destination)
    };

    if !source_path.exists() {
        return Err(anyhow!("Source path does not exist"));
    }

    println!("Scanning '{:?}'...", source_path);
    let scanner = Scanner::new(args.depth);
    let files = scanner.scan(&source_path);

    let mut tasks_by_container: HashMap<String, Vec<Task>> = HashMap::new();
    let mut total_files = 0;

    for file in files {
        let ext = file
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        if ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
            if let Ok(zip_file) = fs::File::open(&file)
                && let Ok(mut archive) = zip::ZipArchive::new(zip_file)
            {
                let rel_path = file.strip_prefix(&source_path).unwrap_or(&file);
                let parent = rel_path.parent().unwrap_or(Path::new(""));
                let stem = file.file_stem().unwrap_or_default();
                let cbz_dest_folder = dest_path.join(parent).join(stem);

                let container_name = file.file_name().unwrap().to_string_lossy().to_string();

                for i in 0..archive.len() {
                    if let Ok(a_file) = archive.by_index(i)
                        && a_file.is_file()
                    {
                        let a_ext = Path::new(a_file.name())
                            .extension()
                            .and_then(|s| s.to_str())
                            .map(|s| s.to_lowercase())
                            .unwrap_or_default();

                        if IMAGE_EXTENSIONS.contains(&a_ext.as_str()) {
                            let target_file = cbz_dest_folder
                                .join(a_file.name())
                                .with_extension(&args.format);

                            tasks_by_container
                                .entry(container_name.clone())
                                .or_default()
                                .push(Task {
                                    src_path: file.clone(),
                                    dest_path: target_file,
                                    task_type: TaskType::Archive {
                                        internal_path: a_file.name().to_string(),
                                    },
                                });
                            total_files += 1;
                        }
                    }
                }
            }
        } else if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            let rel_path = file.strip_prefix(&source_path).unwrap_or(&file);
            let target_file = dest_path.join(rel_path).with_extension(&args.format);

            let parent_folder = rel_path
                .parent()
                .unwrap_or(Path::new("root"))
                .to_string_lossy()
                .to_string();

            tasks_by_container
                .entry(parent_folder.clone())
                .or_default()
                .push(Task {
                    src_path: file,
                    dest_path: target_file,
                    task_type: TaskType::File,
                });
            total_files += 1;
        }
    }

    let container_names: Vec<String> = {
        let mut names: Vec<String> = tasks_by_container.keys().cloned().collect();
        names.sort();
        names
    };

    println!(
        "Found {} containers with {} files in total.",
        container_names.len(),
        total_files
    );

    if tasks_by_container.is_empty() {
        return Ok(());
    }

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
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
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

                    if let Err(e) = process_task(task, &args) {
                        mp.suspend(|| eprintln!("Error processing {}: {}", name_display, e));
                    }

                    spinner.set_message("Idle");
                    spinner.disable_steady_tick();

                    spinner_pool.lock().unwrap().push(spinner);
                } else if let Err(e) = process_task(task, &args) {
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

fn process_task(task: &Task, args: &ImageArgs) -> Result<()> {
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
        .map_err(|e| anyhow!("AVIF encoding failed: {}", e))?;

    fs::write(dest, out.avif_file)?;
    Ok(())
}
