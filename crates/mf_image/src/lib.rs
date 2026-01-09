use anyhow::{Result, anyhow};
use clap::Args as ClapArgs;
use image::{DynamicImage, GenericImageView, ImageFormat};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mf_core::{ARCHIVE_EXTENSIONS, CpuControl, IMAGE_EXTENSIONS, Scanner};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

    /// Number of threads
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

    let mut tasks = Vec::new();
    for file in files {
        let ext = file
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        if ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
            let zip_file = fs::File::open(&file)?;
            let mut archive = zip::ZipArchive::new(zip_file)?;
            let rel_path = file.strip_prefix(&source_path)?;
            let cbz_dest_folder = dest_path
                .join(rel_path.parent().unwrap())
                .join(file.file_stem().unwrap());

            for i in 0..archive.len() {
                let a_file = archive.by_index(i)?;
                if a_file.is_file() {
                    let a_ext = Path::new(a_file.name())
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_lowercase())
                        .unwrap_or_default();
                    if IMAGE_EXTENSIONS.contains(&a_ext.as_str()) {
                        let target_file = cbz_dest_folder
                            .join(a_file.name())
                            .with_extension(&args.format);
                        tasks.push(Task {
                            src_path: file.clone(),
                            dest_path: target_file,
                            task_type: TaskType::Archive {
                                internal_path: a_file.name().to_string(),
                            },
                        });
                    }
                }
            }
        } else if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            let rel_path = file.strip_prefix(&source_path)?;
            let target_file = dest_path.join(rel_path).with_extension(&args.format);
            tasks.push(Task {
                src_path: file,
                dest_path: target_file,
                task_type: TaskType::File,
            });
        }
    }

    println!("Found {} total images.", tasks.len());

    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(tasks.len() as u64));
    pb_main.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")?
            .progress_chars("#>-"),
    );
    pb_main.set_message("Total Progress");

    let start_instant = Instant::now();

    let args = Arc::new(args);
    let pb_main = Arc::new(pb_main);

    tasks.par_iter().for_each(|task| {
        if let Err(e) = process_task(task, &args) {
            eprintln!("Error processing {:?}: {}", task.src_path, e);
        }
        pb_main.inc(1);
    });

    pb_main.finish_with_message("Done!");
    let duration = start_instant.elapsed();
    println!("Completed in {:?}", duration);

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

    if !args.no_mtime && matches!(task.task_type, TaskType::File) {
        let metadata = fs::metadata(&task.src_path)?;
        if let Ok(_mtime) = metadata.modified() {
            // TODO: mtime preservation
        }
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
