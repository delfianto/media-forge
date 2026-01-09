use anyhow::{Result, anyhow};
use clap::Args as ClapArgs;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mf_core::{Scanner, VIDEO_EXTENSIONS};
use rayon::prelude::*;
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// Command-line arguments for video encoding.
///
/// Encodes videos to AV1 format using NVIDIA NVENC hardware acceleration.
/// Requires an NVIDIA GPU with NVENC support (GTX 10-series or newer)
/// and FFmpeg compiled with NVENC support.
#[derive(ClapArgs, Debug, Clone)]
pub struct VideoArgs {
    /// Output directory for encoded videos
    ///
    /// Encoded videos are saved here, preserving the original
    /// directory structure relative to the source path.
    #[arg(value_name = "DEST")]
    pub destination: PathBuf,

    /// Source directory containing videos to encode
    ///
    /// Scans for supported video formats: MP4, MKV, MOV, AVI, TS, M4V.
    /// Videos already encoded in AV1 are automatically skipped.
    #[arg(short, long, default_value = ".", value_name = "DIR")]
    pub source: PathBuf,

    /// Constant quality level (1-51)
    ///
    /// Lower values produce better quality but larger files.
    /// 18-22: Very high quality (archival, visually lossless)
    /// 23-28: High quality (recommended for general use)
    /// 29-35: Medium quality (good for streaming)
    /// 36+: Lower quality (maximum compression)
    #[arg(long, default_value_t = 28, value_name = "1-51")]
    pub cq: u8,

    /// NVIDIA NVENC encoding preset (p1-p7)
    ///
    /// Controls the speed/quality tradeoff of the encoder.
    /// p1: Slowest, best quality (final renders)
    /// p4: Balanced speed and quality
    /// p6: Fast encoding, good quality (default)
    /// p7: Fastest, lower quality (previews)
    #[arg(long, default_value = "p6", value_name = "PRESET")]
    pub preset: String,

    /// Number of concurrent encoding jobs
    ///
    /// Each encoding job uses significant GPU memory. Keep low (1-2)
    /// to avoid GPU memory exhaustion. Increase only if your GPU
    /// has sufficient VRAM for parallel encoding.
    #[arg(short, long, default_value_t = 1, value_name = "N")]
    pub jobs: usize,

    /// Output container format
    ///
    /// MKV: Better subtitle support, more flexible (recommended)
    /// MP4: Wider compatibility, required for some devices
    #[arg(long, default_value = "mkv", value_parser = ["mkv", "mp4"], value_name = "FMT")]
    pub ext: String,

    /// Maximum directory recursion depth
    ///
    /// How many levels of subdirectories to scan for videos.
    /// Use higher values for deeply nested folder structures.
    #[arg(long, default_value_t = 2, value_name = "N")]
    pub depth: usize,
}

#[derive(Debug, Clone)]
struct VideoTask {
    src: PathBuf,
    dest: PathBuf,
    duration: f64,
}

pub fn run(args: VideoArgs) -> Result<()> {
    let source_path = fs::canonicalize(&args.source)?;
    let dest_path = if args.destination.is_absolute() {
        args.destination.clone()
    } else {
        std::env::current_dir()?.join(&args.destination)
    };

    println!("Scanning '{:?}' for videos...", source_path);
    let scanner = Scanner::new(args.depth);
    let files = scanner.scan(&source_path);

    let tasks_mutex = Arc::new(Mutex::new(Vec::new()));

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.jobs.max(4)) // Allow more threads for lightweight probing
        .build()?;

    let pb_scan = ProgressBar::new(files.len() as u64);
    pb_scan.set_style(
        ProgressStyle::default_bar()
            .template("{spinner} Scanning {pos}/{len}")
            .unwrap(),
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
                let rel_path = file.strip_prefix(&source_path).unwrap();
                let dest_file = dest_path.join(rel_path).with_extension(&args.ext);

                if !dest_file.exists() {
                    tasks_mutex.lock().unwrap().push(VideoTask {
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

    let tasks = Arc::try_unwrap(tasks_mutex).unwrap().into_inner().unwrap();
    println!("Found {} videos to process.", tasks.len());

    if tasks.is_empty() {
        return Ok(());
    }

    let mp = MultiProgress::new();
    let pb_main = mp.add(ProgressBar::new(tasks.len() as u64));
    pb_main.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}").unwrap()
            .progress_chars("#>-"),
    );
    pb_main.set_message("Total Video Progress");

    // Processing pool with limited concurrency (heavy GPU usage)
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
        pb_file.set_message(format!(
            "{}",
            task.src.file_name().unwrap().to_string_lossy()
        ));

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
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path.to_str().unwrap(),
        ])
        .output()?;

    let json: Value = serde_json::from_slice(&output.stdout)?;

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
        task.src.to_str().unwrap(),
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

    cmd.arg(task.dest.to_str().unwrap());

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    let stderr = child
        .stderr
        .take()
        .ok_or(anyhow!("Failed to capture stderr"))?;
    let reader = BufReader::new(stderr);

    let re = Regex::new(r"time=(\d+):(\d+):(\d+\.\d+)").unwrap();

    for line in reader.lines().map_while(Result::ok) {
        if let Some(caps) = re.captures(&line) {
            let h: u64 = caps[1].parse().unwrap_or(0);
            let m: u64 = caps[2].parse().unwrap_or(0);
            let s: f64 = caps[3].parse().unwrap_or(0.0);
            let seconds = (h * 3600 + m * 60) as f64 + s;
            pb.set_position(seconds as u64);
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(anyhow!("ffmpeg exited with {}", status));
    }

    if let Ok(meta) = fs::metadata(&task.src)
        && let Ok(mtime) = meta.modified()
    {
        let file = fs::File::open(&task.dest)?;
        file.set_modified(mtime).ok();
    }

    Ok(())
}
