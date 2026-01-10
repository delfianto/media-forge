use crate::{Naming, PathUtil, ProcessManager, SHUTDOWN, Scanner, VIDEO_EXTENSIONS, ui};
use indicatif::{MultiProgress, ProgressBar};
use rayon::prelude::*;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::video::{FFMPEG_PROGRESS_RE, Result, VideoArgs, VideoError, get_video_metadata};

/// Represents a single video encoding task.
#[derive(Debug, Clone)]
pub struct VideoTask {
    /// Path to the source video file.
    pub src: PathBuf,
    /// Path to the destination video file.
    pub dest: PathBuf,
    /// Duration of the video in seconds.
    pub duration: f64,
}

/// Main entry point for video encoding orchestration.
///
/// This function verifies dependencies, scans for videos, collects tasks,
/// and executes them in parallel using the specified number of threads.
pub fn run(args: VideoArgs) -> anyhow::Result<()> {
    check_encoding_requirements()?;

    let (source_path, dest_path) = PathUtil::resolve_paths(&args.source, &args.destination)
        .map_err(|_| VideoError::SourceNotFound(args.source.clone()))?;

    let scanner = Scanner::new(args.depth);
    let files = if source_path.is_file() {
        vec![source_path.clone()]
    } else {
        println!("Scanning {} for videos...", source_path.display());
        scanner.scan_with_progress(&source_path, "Scanning...")
    };

    let tasks = collect_video_tasks(files, &source_path, &dest_path, &args)?;

    println!("Found {} videos to process.", tasks.len());
    if tasks.is_empty() {
        return Ok(());
    }

    process_video_tasks(tasks, args)?;

    Ok(())
}

/// Verifies that FFmpeg and ffprobe are available in the system PATH.
fn check_encoding_requirements() -> Result<()> {
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

/// Collects valid video tasks from the list of files.
///
/// Filters for valid video extensions, checks metadata, and skips files
/// based on overwrite settings and existing output.
fn collect_video_tasks(
    files: Vec<PathBuf>,
    source_path: &Path,
    dest_path: &Path,
    args: &VideoArgs,
) -> Result<Vec<VideoTask>> {
    let tasks_mutex = Arc::new(Mutex::new(Vec::new()));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads((num_cpus::get() / 2).max(4))
        .build()?;

    let pb_scan = ProgressBar::new(files.len() as u64);
    pb_scan.set_style(ui::main_bar_style());

    pool.install(|| {
        files.par_iter().for_each(|file| {
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            pb_scan.set_message(name.to_string());

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
                let dest_file = if source_path.is_file() {
                    if dest_path.extension().is_some() {
                        dest_path.to_path_buf()
                    } else {
                        dest_path
                            .join(file.file_name().unwrap())
                            .with_extension(&args.ext)
                    }
                } else {
                    let rel_path = file.strip_prefix(source_path).unwrap_or(file);
                    dest_path.join(rel_path).with_extension(&args.ext)
                };

                if !PathUtil::should_skip(&dest_file, args.overwrite)
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

    let tasks = (Arc::try_unwrap(tasks_mutex)
        .map_err(|_| VideoError::GpuError("Failed to unwrap task list".into()))?)
    .into_inner()
    .map_err(|_| VideoError::GpuError("Failed to unlock task list".into()))?;

    Ok(tasks)
}

fn process_video_tasks(tasks: Vec<VideoTask>, args: VideoArgs) -> Result<()> {
    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(tasks.len() as u64));
    pb_main.set_style(ui::main_bar_style());
    pb_main.set_message("Total Video Progress");

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.jobs)
        .build()
        .map_err(VideoError::from)?;
    let args = Arc::new(args);
    let pb_main = Arc::new(pb_main);
    let start_time = Instant::now();

    pool.install(|| {
        tasks.par_iter().for_each(|task| {
            let pb_file = mp.add(ProgressBar::new(task.duration as u64));
            pb_file.set_style(ui::file_progress_style());

            let name = task
                .src
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let name_display = Naming::truncate_from_start(&name, 50);

            pb_file.set_message(name_display);

            if let Err(e) = convert_video(task, &args, &pb_file) {
                mp.suspend(|| eprintln!("Error converting {:?}: {}", task.src, e));
            }

            pb_file.finish_and_clear();
            mp.remove(&pb_file);
            pb_main.inc(1);
        });
    });

    pb_main.finish_with_message("Done!");
    println!("Total time: {:.2?}", start_time.elapsed());
    Ok(())
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
        "error",
        "-stats",
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

    if SHUTDOWN.load(Ordering::SeqCst) {
        return Ok(());
    }

    let mut child = cmd.spawn()?;
    let pid = child.id();
    ProcessManager::register(pid);

    let stderr = child
        .stderr
        .take()
        .ok_or(VideoError::ProcessOutputCaptureFailed)?;
    let mut reader = BufReader::new(stderr);
    let mut buffer = Vec::new();

    while reader.read_until(b'\r', &mut buffer)? > 0 {
        if SHUTDOWN.load(Ordering::SeqCst) {
            let _ = child.kill();
            break;
        }

        let line = String::from_utf8_lossy(&buffer);
        if let Some(caps) = FFMPEG_PROGRESS_RE.captures(&line) {
            let h: u64 = caps[1].parse().unwrap_or(0);
            let m: u64 = caps[2].parse().unwrap_or(0);
            let s: f64 = caps[3].parse().unwrap_or(0.0);
            let seconds = (h * 3600 + m * 60) as f64 + s;
            pb.set_position(seconds as u64);
        }
        buffer.clear();
    }

    let status = child.wait()?;
    ProcessManager::unregister(pid);

    if !status.success() && !SHUTDOWN.load(Ordering::SeqCst) {
        return Err(VideoError::FfmpegFailed {
            code: status.code().unwrap_or(-1),
            stderr: "Encoding failed. Check GPU memory or source file.".to_string(),
        });
    }

    if let Ok(metadata) = fs::metadata(&task.src)
        && let Ok(mtime) = metadata.modified()
    {
        let file = fs::File::open(&task.dest)?;
        file.set_modified(mtime).ok();
    }

    Ok(())
}
