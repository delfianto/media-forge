use clap::Args as ClapArgs;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mf_core::{Scanner, VIDEO_EXTENSIONS};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Video processing errors with context-specific information.
#[derive(Error, Debug)]
pub enum VideoError {
    #[error("FFmpeg not found. Install FFmpeg with NVENC support (av1_nvenc codec)")]
    FfmpegNotFound,

    #[error("ffprobe not found. Install FFmpeg which includes ffprobe")]
    FfprobeNotFound,

    #[error("FFmpeg exited with code {code}. Stderr: {stderr}")]
    FfmpegFailed { code: i32, stderr: String },

    #[error("Failed to parse video metadata from {path:?}: {reason}")]
    MetadataParseError { path: PathBuf, reason: String },

    #[error("GPU encoding error: {0}. Try reducing --jobs or check GPU memory")]
    GpuError(String),

    #[error("Failed to capture FFmpeg output stream")]
    ProcessOutputCaptureFailed,

    #[error("Invalid path: contains non-UTF8 characters: {0:?}")]
    InvalidPath(PathBuf),

    #[error("Source path not found: {0:?}")]
    SourceNotFound(PathBuf),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Thread pool error: {0}")]
    ThreadPoolError(#[from] rayon::ThreadPoolBuildError),

    #[error("Template error: {0}")]
    Template(#[from] indicatif::style::TemplateError),

    #[error("Could not determine current directory")]
    NoCurrentDir,
}

pub type Result<T> = std::result::Result<T, VideoError>;

/// Cached regex for parsing FFmpeg progress output.
static FFMPEG_PROGRESS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"time=(\d+):(\d+):(\d+\.\d+)")
        .expect("FFmpeg progress regex is hardcoded and should always compile")
});

/// Command-line arguments for video encoding.
#[derive(ClapArgs, Debug, Clone)]
pub struct VideoArgs {
    /// Output directory for encoded videos
    #[arg(value_name = "DEST")]
    pub destination: PathBuf,

    /// Source directory containing videos to encode
    #[arg(short, long, default_value = ".", value_name = "DIR")]
    pub source: PathBuf,

    /// Constant quality level (1-51)
    #[arg(long, default_value_t = 28, value_name = "1-51")]
    pub cq: u8,

    /// NVIDIA NVENC encoding preset (p1-p7)
    #[arg(long, default_value = "p6", value_name = "PRESET")]
    pub preset: String,

    /// Number of concurrent encoding jobs
    #[arg(short, long, default_value_t = 1, value_name = "N")]
    pub jobs: usize,

    /// Output container format
    #[arg(long, default_value = "mkv", value_parser = ["mkv", "mp4"], value_name = "FMT")]
    pub ext: String,

    /// Maximum directory recursion depth
    #[arg(long, default_value_t = 2, value_name = "N")]
    pub depth: usize,
}

#[derive(Debug, Clone)]
struct VideoTask {
    src: PathBuf,
    dest: PathBuf,
    duration: f64,
}

/// Main entry point for video encoding.
pub fn run(args: VideoArgs) -> anyhow::Result<()> {
    // Check requirements
    check_requirements()?;

    // Resolve paths
    let source_path = fs::canonicalize(&args.source)
        .map_err(|_| VideoError::SourceNotFound(args.source.clone()))?;

    let dest_path = if args.destination.is_absolute() {
        args.destination.clone()
    } else {
        std::env::current_dir()
            .map_err(|_| VideoError::NoCurrentDir)?
            .join(&args.destination)
    };

    println!("Scanning {} for videos...", source_path.display());
    let pb_scan_dir = ProgressBar::new_spinner();
    pb_scan_dir.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg} {pos} items found")
            .unwrap(),
    );
    pb_scan_dir.enable_steady_tick(std::time::Duration::from_millis(100));

    let mut items_found = 0;
    let scanner = Scanner::new(args.depth);
    let files = scanner.scan_with_callback(&source_path, |path| {
        items_found += 1;
        pb_scan_dir.set_position(items_found);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        pb_scan_dir.set_message(format!("Scanning: {}", name));
    });
    pb_scan_dir.finish_and_clear();

    // Discover tasks
    let tasks = collect_video_tasks(files, &source_path, &dest_path, &args)?;

    println!("Found {} videos to process.", tasks.len());
    if tasks.is_empty() {
        return Ok(());
    }

    // Execute tasks
    process_video_tasks(tasks, args)?;

    Ok(())
}

/// Verifies that ffmpeg and ffprobe are available.
fn check_requirements() -> Result<()> {
    if Command::new("ffprobe")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return Err(VideoError::FfprobeNotFound);
    }
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return Err(VideoError::FfmpegNotFound);
    }
    Ok(())
}

/// Collects video encoding tasks in parallel.
fn collect_video_tasks(
    files: Vec<PathBuf>,
    source_path: &Path,
    dest_path: &Path,
    args: &VideoArgs,
) -> Result<Vec<VideoTask>> {
    let tasks_mutex = Arc::new(Mutex::new(Vec::new()));

    // Use a temporary thread pool for scanning to avoid interfering with global pool
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads((num_cpus::get() / 2).max(4))
        .build()?;

    let pb_scan = ProgressBar::new(files.len() as u64);
    pb_scan.set_style(
        ProgressStyle::default_bar()
            .template("{spinner} Scanning {pos}/{len}")
            .map_err(VideoError::from)?,
    );

    pool.install(|| {
        files.par_iter().for_each(|file| {
            let ext = file
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();

            if VIDEO_EXTENSIONS.contains(&ext.as_str())
                && let Ok(meta) = get_video_metadata(file)
                && meta.codec != "av1"
                && meta.duration > 0.0
            {
                let rel_path = file.strip_prefix(source_path).unwrap_or(file);
                let dest_file = dest_path.join(rel_path).with_extension(&args.ext);

                if !dest_file.exists()
                    && let Ok(mut tasks) = tasks_mutex.lock()
                {
                    tasks.push(VideoTask {
                        src: file.clone(),
                        dest: dest_file,
                        duration: meta.duration,
                    });
                }
            }
            pb_scan.inc(1);
        });
    });
    pb_scan.finish_and_clear();

    let tasks = Arc::try_unwrap(tasks_mutex)
        .map_err(|_| VideoError::GpuError("Failed to unwrap task list".into()))?
        .into_inner()
        .map_err(|_| VideoError::GpuError("Failed to unlock task list".into()))?;

    Ok(tasks)
}

/// Executes video encoding tasks with progress tracking.
fn process_video_tasks(tasks: Vec<VideoTask>, args: VideoArgs) -> Result<()> {
    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(tasks.len() as u64));
    pb_main.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")?
            .progress_chars("#>-"),
    );
    pb_main.set_message("Total Video Progress");

    // Re-configure global thread pool for limited concurrency (heavy GPU usage)
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(args.jobs)
        .build_global();

    let args = Arc::new(args);
    let pb_main = Arc::new(pb_main);

    tasks.par_iter().for_each(|task| {
        let pb_file = mp.add(ProgressBar::new(task.duration as u64));
        pb_file.set_style(
            ProgressStyle::default_bar()
                .template("{msg} {bar:20} {percent}%")
                .unwrap(),
        );
        pb_file.set_message(
            task.src
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        );

        if let Err(e) = convert_video(task, &args, &pb_file) {
            eprintln!("Error converting {:?}: {}", task.src, e);
        }

        mp.remove(&pb_file);
        pb_main.inc(1);
    });

    pb_main.finish_with_message("Done!");
    Ok(())
}

struct VideoMeta {
    duration: f64,
    codec: String,
}

fn get_video_metadata(path: &Path) -> Result<VideoMeta> {
    let path_str = path
        .to_str()
        .ok_or_else(|| VideoError::InvalidPath(path.to_path_buf()))?;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path_str,
        ])
        .output()?;

    if !output.status.success() {
        return Err(VideoError::MetadataParseError {
            path: path.to_path_buf(),
            reason: format!(
                "ffprobe failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| VideoError::MetadataParseError {
            path: path.to_path_buf(),
            reason: format!("Invalid JSON: {}", e),
        })?;

    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    let streams = json["streams"].as_array();
    let codec = streams
        .and_then(|arr| arr.iter().find(|s| s["codec_type"] == "video"))
        .and_then(|s| s["codec_name"].as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(VideoMeta { duration, codec })
}

fn convert_video(task: &VideoTask, args: &VideoArgs, pb: &ProgressBar) -> Result<()> {
    if let Some(parent) = task.dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let src_str = task
        .src
        .to_str()
        .ok_or_else(|| VideoError::InvalidPath(task.src.clone()))?;
    let dest_str = task
        .dest
        .to_str()
        .ok_or_else(|| VideoError::InvalidPath(task.dest.clone()))?;

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-y",
        "-hide_banner",
        "-loglevel",
        "info",
        "-hwaccel",
        "cuda",
        "-hwaccel_output_format",
        "cuda",
        "-i",
        src_str,
        "-c:v",
        "av1_nvenc",
        "-preset",
        &args.preset,
        "-tune",
        "hq",
        "-cq",
        &args.cq.to_string(),
        "-c:a",
        "copy",
    ]);

    if args.ext == "mp4" {
        cmd.args(["-c:s", "mov_text"]);
    } else {
        cmd.args(["-c:s", "copy"]);
    }

    cmd.arg(dest_str);

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    let stderr = child
        .stderr
        .take()
        .ok_or(VideoError::ProcessOutputCaptureFailed)?;
    let reader = BufReader::new(stderr);

    for line in reader.lines().map_while(std::result::Result::ok) {
        if let Some(caps) = FFMPEG_PROGRESS_RE.captures(&line) {
            let h: u64 = caps[1].parse().unwrap_or(0);
            let m: u64 = caps[2].parse().unwrap_or(0);
            let s: f64 = caps[3].parse().unwrap_or(0.0);
            let seconds = (h * 3600 + m * 60) as f64 + s;
            pb.set_position(seconds as u64);
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(VideoError::FfmpegFailed {
            code: status.code().unwrap_or(-1),
            stderr: "Check ffmpeg logs for details".to_string(),
        });
    }

    if let Ok(meta) = fs::metadata(&task.src)
        && let Ok(mtime) = meta.modified()
    {
        let file = fs::File::open(&task.dest)?;
        file.set_modified(mtime).ok();
    }

    Ok(())
}
