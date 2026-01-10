use clap::Args as ClapArgs;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

pub mod builder;
pub mod encode;
pub mod quality;

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

    #[error("VMAF analysis failed: {0}")]
    VmafError(String),

    #[error("VMAF filter not found in FFmpeg. Ensure FFmpeg is built with --enable-libvmaf")]
    VmafFilterNotFound,
}

/// Convenience result type using VideoError.
pub type Result<T> = std::result::Result<T, VideoError>;

/// Cached regex for parsing FFmpeg progress output.
pub static FFMPEG_PROGRESS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"time=(\d+):(\d+):(\d+\.\d+)")
        .expect("FFmpeg progress regex is hardcoded and should always compile")
});

/// Cached regex for parsing VMAF scores from FFmpeg output.
pub static VMAF_SCORE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)VMAF score:\s*([\d.]+)|mean:\s*([\d.]+).*?min:\s*([\d.]+).*?max:\s*([\d.]+)")
        .expect("VMAF score regex is hardcoded and should always compile")
});

use crate::constants::{DEFAULT_RECURSION_DEPTH, DEFAULT_VIDEO_CQ, DEFAULT_VMAF_DURATION};

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
    #[arg(long, default_value_t = DEFAULT_VIDEO_CQ, value_name = "1-51")]
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
    #[arg(long, default_value_t = DEFAULT_RECURSION_DEPTH, value_name = "N")]
    pub depth: usize,

    /// Overwrite existing files even if they are not empty
    #[arg(short, long, alias = "override")]
    pub overwrite: bool,
}

/// Command-line arguments for video quality analysis (VMAF).
#[derive(ClapArgs, Debug, Clone)]
pub struct QualityArgs {
    /// Original reference video
    #[arg(value_name = "ORIGINAL")]
    pub original: PathBuf,

    /// Encoded video file to evaluate
    #[arg(value_name = "ENCODED")]
    pub encoded: PathBuf,

    /// Save detailed detailed VMAF results as JSON
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Duration to analyze in seconds
    #[arg(short, long, default_value_t = DEFAULT_VMAF_DURATION, value_name = "SECONDS")]
    pub duration: u64,

    /// Start time for analysis in seconds
    #[arg(short, long, default_value_t = 0, value_name = "SECONDS")]
    pub start: u64,

    /// Analyze entire video
    #[arg(long)]
    pub full: bool,

    /// Number of threads for VMAF calculation (0=auto)
    #[arg(short, long, default_value_t = 0, value_name = "N")]
    pub threads: usize,

    /// Analyze every Nth frame
    #[arg(long, default_value_t = 1, value_name = "N")]
    pub subsample: usize,

    /// Downscale to height for faster analysis
    #[arg(long, default_value = "1080", value_parser = ["480", "720", "1080"], value_name = "HEIGHT")]
    pub scale: String,
}

/// Minimal video metadata required for task planning.
pub struct VideoMeta {
    /// Duration in seconds.
    pub duration: f64,
    /// Name of the video codec.
    pub codec: String,
    /// Video width in pixels.
    pub width: u32,
    /// Video height in pixels.
    pub height: u32,
}

/// Retrieves video metadata using ffprobe.
pub fn get_video_metadata(path: &Path) -> Result<VideoMeta> {
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
    let video_stream = streams.and_then(|arr| arr.iter().find(|s| s["codec_type"] == "video"));

    let codec = video_stream
        .and_then(|s| s["codec_name"].as_str())
        .unwrap_or("unknown")
        .to_string();

    let width = video_stream.and_then(|s| s["width"].as_u64()).unwrap_or(0) as u32;

    let height = video_stream.and_then(|s| s["height"].as_u64()).unwrap_or(0) as u32;

    Ok(VideoMeta {
        duration,
        codec,
        width,
        height,
    })
}

/// Delegates video encoding to the internal module.
pub fn run(args: VideoArgs) -> anyhow::Result<()> {
    encode::run(args)
}

/// Delegates quality analysis to the internal module.
pub fn run_quality(args: QualityArgs) -> anyhow::Result<()> {
    quality::run_quality(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffmpeg_progress_regex() {
        let line = "frame=  123 fps= 12 q=-0.0 size=    1234kB time=00:01:23.45 bitrate= 123.4kbits/s speed=1.23x";
        let caps = FFMPEG_PROGRESS_RE.captures(line).unwrap();
        assert_eq!(&caps[1], "00");
        assert_eq!(&caps[2], "01");
        assert_eq!(&caps[3], "23.45");
    }

    #[test]
    fn test_vmaf_score_regex() {
        let line1 = "VMAF score: 95.123456";
        let caps1 = VMAF_SCORE_RE.captures(line1).unwrap();
        assert_eq!(&caps1[1], "95.123456");

        let line2 = "VMAF score: mean: 92.500000 min: 80.100000 max: 99.000000";
        let caps2 = VMAF_SCORE_RE.captures(line2).unwrap();
        assert_eq!(&caps2[2], "92.500000");
        assert_eq!(&caps2[3], "80.100000");
        assert_eq!(&caps2[4], "99.000000");
    }
}
