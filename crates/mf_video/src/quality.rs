use indicatif::{ProgressBar, ProgressStyle};
use mf_core::{ProcessManager, SHUTDOWN};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

use crate::{
    FFMPEG_PROGRESS_RE, QualityArgs, Result, VMAF_SCORE_RE, VideoError, get_video_metadata,
};

/// Scores returned by VMAF analysis.
pub struct VmafScores {
    /// Mean VMAF score (0-100).
    pub mean: f64,
    /// Minimum VMAF score.
    pub min: f64,
    /// Maximum VMAF score.
    pub max: f64,
}

/// Entry point for video quality analysis orchestration.
pub fn run_quality(args: QualityArgs) -> anyhow::Result<()> {
    check_vmaf_requirements()?;

    if !args.original.exists() {
        return Err(VideoError::SourceNotFound(args.original).into());
    }
    if !args.encoded.exists() {
        return Err(VideoError::SourceNotFound(args.encoded).into());
    }

    let meta = get_video_metadata(&args.encoded)?;
    let analysis_duration = if args.full {
        meta.duration
    } else {
        args.duration
            .map(|d| d as f64)
            .unwrap_or(600.0)
            .min(meta.duration - args.start as f64)
    };

    println!("\n{}", "=".repeat(50));
    println!("Original: {}", args.original.display());
    println!("Encoded:  {}", args.encoded.display());
    println!(
        "Duration: {:.2}s (starting at {}s)",
        analysis_duration, args.start
    );
    println!("{}\n", "=".repeat(50));

    let pb = ProgressBar::new(analysis_duration as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} VMAF Analysis: [{bar:40.cyan/blue}] {pos}/{len}s ({eta})")?
            .progress_chars("#>- "),
    );

    let vmaf_scores = calculate_vmaf(&args, analysis_duration, &pb)?;
    pb.finish_and_clear();

    display_vmaf_results(&vmaf_scores);
    Ok(())
}

/// Verifies that FFmpeg supports the VMAF filter.
fn check_vmaf_requirements() -> Result<()> {
    let output = Command::new("ffmpeg").arg("-filters").output()?;
    let filters = String::from_utf8_lossy(&output.stdout);
    if !filters.contains("libvmaf") {
        return Err(VideoError::VmafFilterNotFound);
    }
    Ok(())
}

/// Executes the VMAF calculation using FFmpeg.
fn calculate_vmaf(args: &QualityArgs, duration: f64, pb: &ProgressBar) -> Result<VmafScores> {
    let mut vmaf_opts = Vec::new();
    let threads = if args.threads == 0 {
        (num_cpus::get() as i32 - 2).max(1) as usize
    } else {
        args.threads
    };
    vmaf_opts.push(format!("n_threads={}", threads));

    if args.subsample > 1 {
        vmaf_opts.push(format!("n_subsample={}", args.subsample));
    }
    if let Some(ref output_json) = args.output {
        vmaf_opts.push("log_fmt=json".to_string());
        vmaf_opts.push(format!("log_path='{}'", output_json.display()));
    }

    let scale_filter = if let Some(ref h) = args.scale {
        format!("scale=-2:{},", h)
    } else {
        "".to_string()
    };
    let vmaf_filter = format!(
        "[0:v]{}setpts=PTS-STARTPTS[dist];[1:v]{}setpts=PTS-STARTPTS[ref];[dist][ref]libvmaf={}",
        scale_filter,
        scale_filter,
        vmaf_opts.join(":")
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-stats"]);
    if args.start > 0 {
        cmd.args(["-ss", &args.start.to_string()]);
    }
    cmd.args(["-t", &duration.to_string()]);
    cmd.args([
        "-i",
        args.encoded.to_str().unwrap(),
        "-i",
        args.original.to_str().unwrap(),
        "-lavfi",
        &vmaf_filter,
        "-f",
        "null",
        "-",
    ]);

    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let pid = child.id();
    ProcessManager::register(pid);

    let stderr = child
        .stderr
        .take()
        .ok_or(VideoError::ProcessOutputCaptureFailed)?;
    let mut reader = BufReader::new(stderr);
    let mut buffer = Vec::new();
    let mut output_accumulator = String::new();

    while reader.read_until(b'\r', &mut buffer)? > 0 {
        if SHUTDOWN.load(Ordering::SeqCst) {
            let _ = child.kill();
            break;
        }
        let line = String::from_utf8_lossy(&buffer);
        output_accumulator.push_str(&line);

        if let Some(caps) = FFMPEG_PROGRESS_RE.captures(&line) {
            let h: u64 = caps[1].parse().unwrap_or(0);
            let m: u64 = caps[2].parse().unwrap_or(0);
            let s: f64 = caps[3].parse().unwrap_or(0.0);
            let current_seconds = (h * 3600 + m * 60) as f64 + s;
            pb.set_position(current_seconds as u64);
        }
        buffer.clear();
    }

    let mut remaining = String::new();
    reader.read_to_string(&mut remaining)?;
    output_accumulator.push_str(&remaining);

    let status = child.wait()?;
    ProcessManager::unregister(pid);

    if !status.success() && !SHUTDOWN.load(Ordering::SeqCst) {
        return Err(VideoError::VmafError(
            "FFmpeg failed during VMAF analysis".into(),
        ));
    }

    let scores = VMAF_SCORE_RE
        .captures(&output_accumulator)
        .map(|caps| VmafScores {
            mean: caps[1].parse().unwrap_or(0.0),
            min: caps[2].parse().unwrap_or(0.0),
            max: caps[3].parse().unwrap_or(0.0),
        })
        .ok_or_else(|| VideoError::VmafError("Could not parse VMAF scores from output".into()))?;

    Ok(scores)
}

/// Formats and displays VMAF analysis results.
fn display_vmaf_results(scores: &VmafScores) {
    let (rating, color) = if scores.mean >= 95.0 {
        ("Excellent", "\x1b[92m")
    } else if scores.mean >= 90.0 {
        ("Very Good", "\x1b[92m")
    } else if scores.mean >= 80.0 {
        ("Good", "\x1b[93m")
    } else if scores.mean >= 70.0 {
        ("Fair", "\x1b[93m")
    } else {
        ("Poor", "\x1b[91m")
    };

    let reset = "\x1b[0m";
    println!("\n{}", "=".repeat(50));
    println!("VMAF Results:");
    println!(
        "  Mean Score: {}{:.2}{} ({})",
        color, scores.mean, reset, rating
    );
    println!("  Min Score:  {:.2}", scores.min);
    println!("  Max Score:  {:.2}", scores.max);
    println!("{}\n", "=".repeat(50));
}
