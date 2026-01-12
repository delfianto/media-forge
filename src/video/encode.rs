use crate::walker::{Asset, MediaSource, Walker};
use crate::{Naming, PathUtil, ProcessManager, SHUTDOWN, VIDEO_EXTENSIONS, ui};
use indicatif::{MultiProgress, ProgressBar};
use rayon::prelude::*;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::video::builder::{FfmpegCommandBuilder, HwAccel, VideoCodec};
use crate::video::{
    FFMPEG_PROGRESS_RE, Result, VideoArgs, VideoError, VideoSummary, get_video_metadata,
};

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
pub fn run(args: VideoArgs) -> anyhow::Result<()> {
    check_encoding_requirements()?;

    let (source_path, dest_path) = PathUtil::resolve_paths(&args.source, &args.destination)
        .map_err(|_| VideoError::SourceNotFound(args.source.clone()))?;

    let walker = Walker::new(VIDEO_EXTENSIONS, args.depth, true);
    let assets = if source_path.is_file() {
        walker.scan_flat(&source_path)
    } else {
        println!("Scanning {} for videos...", source_path.display());
        walker.scan_with_progress(&source_path, "Scanning...")
    };

    let (tasks, skipped) = collect_video_tasks(assets, &source_path, &dest_path, &args)?;

    println!("Found {} videos to process.", tasks.len());
    if tasks.is_empty() && skipped == 0 {
        return Ok(());
    }

    // Calculate original size
    let original_size: u64 = tasks
        .par_iter()
        .map(|t| fs::metadata(&t.src).map(|m| m.len()).unwrap_or(0))
        .sum();

    let failed = if !tasks.is_empty() {
        process_video_tasks(tasks.clone(), args)?
    } else {
        Vec::new()
    };

    // Calculate final size
    let final_size: u64 = tasks
        .par_iter()
        .map(|t| {
            if t.dest.exists() {
                fs::metadata(&t.dest).map(|m| m.len()).unwrap_or(0)
            } else {
                0
            }
        })
        .sum();

    let summary = VideoSummary {
        total: tasks.len() + skipped,
        succeeded: tasks.len() - failed.len(),
        skipped,
        failed,
        original_size,
        final_size,
    };

    summary.print_summary();

    if summary.exit_code() != 0 {
        return Err(anyhow::anyhow!("Some encodings failed"));
    }

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
    assets: Vec<Asset>,
    source_path: &Path,
    dest_path: &Path,
    args: &VideoArgs,
) -> Result<(Vec<VideoTask>, usize)> {
    let tasks_mutex = Arc::new(Mutex::new(Vec::new()));
    let skipped_counter = std::sync::atomic::AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads((num_cpus::get() / 2).max(4))
        .build()?;

    let pb_scan = ProgressBar::new(assets.len() as u64);
    pb_scan.set_style(ui::main_bar_style());

    pool.install(|| {
        assets.par_iter().for_each(|asset| {
            if let MediaSource::Filesystem(file) = &asset.source {
                let name = file
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                pb_scan.set_message(name.to_string());

                if let Ok(meta) = get_video_metadata(file)
                    && meta.codec != "av1"
                    && meta.duration > 0.0
                {
                    let dest_file = if source_path.is_file() {
                        if dest_path.extension().is_some() {
                            dest_path.to_path_buf()
                        } else {
                            dest_path
                                .join(file.file_name().unwrap_or_default())
                                .with_extension(&args.ext)
                        }
                    } else {
                        let rel_path = file.strip_prefix(source_path).unwrap_or(file);
                        dest_path.join(rel_path).with_extension(&args.ext)
                    };

                    if PathUtil::should_skip(&dest_file, args.overwrite) {
                        skipped_counter.fetch_add(1, Ordering::SeqCst);
                    } else if let Ok(mut tasks) = tasks_mutex.lock() {
                        tasks.push(VideoTask {
                            src: file.clone(),
                            dest: dest_file,
                            duration: meta.duration,
                        });
                    }
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

    Ok((tasks, skipped_counter.load(Ordering::SeqCst)))
}

/// Orchestrates the parallel execution of video encoding tasks.
fn process_video_tasks(tasks: Vec<VideoTask>, args: VideoArgs) -> Result<Vec<(PathBuf, String)>> {
    let mp = MultiProgress::new();

    let pb_main = mp.add(ProgressBar::new(tasks.len() as u64));
    pb_main.set_style(ui::main_bar_style());
    pb_main.set_message("Total Video Progress");

    let mut spinners = Vec::new();
    for _ in 0..args.jobs.min(crate::constants::MAX_ANALYSIS_SPINNERS) {
        let pb = mp.add(ProgressBar::new(0));
        pb.set_style(ui::file_progress_style());
        pb.set_message("Idle");
        spinners.push(pb);
    }
    let spinner_pool = Arc::new(Mutex::new(spinners));

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.jobs)
        .build()
        .map_err(VideoError::from)?;
    let args = Arc::new(args);
    let pb_main = Arc::new(pb_main);
    let start_time = Instant::now();

    let failures: Arc<Mutex<Vec<(PathBuf, String)>>> = Arc::new(Mutex::new(Vec::new()));

    pool.install(|| {
        tasks.par_iter().for_each(|task| {
            let pb_opt = spinner_pool.lock().ok().and_then(|mut pool| pool.pop());

            // Prepare the progress bat to use (either a visible recycled one, or a hidden dummy)
            let pb_to_use = if let Some(ref pb) = pb_opt {
                pb.set_length(task.duration as u64);
                pb.set_position(0);

                let name = task
                    .src
                    .file_name()
                    .map(|n: &std::ffi::OsStr| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let name_display = Naming::truncate_from_start(&name, 50);

                pb.set_message(name_display);
                pb.clone()
            } else {
                ProgressBar::hidden()
            };

            if let Err(e) = convert_video(task, &args, &pb_to_use) {
                mp.suspend(|| eprintln!("Error converting {:?}: {}", task.src, e));
                if let Ok(mut f) = failures.lock() {
                    f.push((task.src.clone(), e.to_string()));
                }
            }

            // Cleanup and return to pool if it was a visible spinner
            if let Some(pb) = pb_opt {
                pb.set_message("Idle");
                pb.set_length(0);
                pb.set_position(0);

                if let Ok(mut pool) = spinner_pool.lock() {
                    pool.push(pb);
                }
            }

            pb_main.inc(1);
        });
    });

    // Cleanup all spinners in the pool
    if let Ok(pool) = spinner_pool.lock() {
        for pb in pool.iter() {
            pb.finish_and_clear();
        }
    }

    pb_main.finish_with_message("Done!");
    println!("Total time: {:.2?}", start_time.elapsed());

    let f = Arc::try_unwrap(failures)
        .map_err(|_| VideoError::GpuError("Failed to unwrap failures".into()))?
        .into_inner()
        .map_err(|_| VideoError::GpuError("Failed to unlock failures".into()))?;

    Ok(f)
}

/// Spawns an FFmpeg process to convert a single video file and tracks its progress.
fn convert_video(task: &VideoTask, args: &VideoArgs, pb: &ProgressBar) -> Result<()> {
    if let Some(parent) = task.dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut builder = FfmpegCommandBuilder::new(&task.src, &task.dest)
        .hwaccel(HwAccel::Cuda)
        .video_codec(VideoCodec::Av1Nvenc)
        .preset(&args.preset)
        .cq(args.cq)
        .args(["-tune", "hq", "-c:a", "copy"]);

    if args.ext == "mp4" {
        builder = builder.args(["-c:s", "mov_text"]);
    } else {
        builder = builder.args(["-c:s", "copy"]);
    }

    let mut cmd = builder.build();

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
