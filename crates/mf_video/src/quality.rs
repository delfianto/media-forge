use indicatif::{ProgressBar, ProgressStyle};
use mf_core::{ProcessManager, SHUTDOWN};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

use crate::{
    FFMPEG_PROGRESS_RE, QualityArgs, Result, VMAF_SCORE_RE, VideoError, VideoMeta,
    get_video_metadata,
};

pub struct VmafScores {
    pub mean: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

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
        (args.duration as f64).min(meta.duration - args.start as f64)
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
            .template("{spinner:.green} VMAF Analysis: [{bar:40.cyan/blue}] {pos}/{len}s ({eta})")? // Corrected: Removed unnecessary escaping of '{' and '}'
            .progress_chars("#>- "),
    );

    let vmaf_scores = calculate_vmaf(&args, &meta, analysis_duration, &pb)?;
    pb.finish_and_clear();

    display_vmaf_results(&vmaf_scores);
    Ok(())
}

fn check_vmaf_requirements() -> Result<()> {
    let output = Command::new("ffmpeg").arg("-filters").output()?;
    let filters = String::from_utf8_lossy(&output.stdout);
    if !filters.contains("libvmaf") {
        return Err(VideoError::VmafFilterNotFound);
    }
    Ok(())
}

fn calculate_vmaf(
    args: &QualityArgs,
    meta: &VideoMeta,
    duration: f64,
    pb: &ProgressBar,
) -> Result<VmafScores> {
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

    let mut model_version = "vmaf_v0.6.1";
    let mut needs_scale = true;

    let is_4k_source = meta.height >= 2160 || meta.width >= 3840;
    if is_4k_source {
        let requested_scale = args.scale.parse::<u32>().unwrap_or(0);
        if requested_scale == 0 || requested_scale >= 2160 {
            model_version = "vmaf_4k_v0.6.1";
            needs_scale = false;
        }
    }
    vmaf_opts.push(format!("model=version={}", model_version));

    if let Some(ref output_json) = args.output {
        vmaf_opts.push("log_fmt=json".to_string());
        vmaf_opts.push(format!("log_path='{}'", output_json.display()));
    }

    let scale_filter = if needs_scale && args.scale != "0" {
        format!("scale_cuda=-2:{},", args.scale)
    } else {
        String::new()
    };

    let filter_chain = format!(
        "[0:v]{}hwdownload,format=nv12,setpts=PTS-STARTPTS[main];         [1:v]{}hwdownload,format=nv12,setpts=PTS-STARTPTS[ref];         [main][ref]libvmaf={}",
        scale_filter,
        scale_filter,
        vmaf_opts.join(":")
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-stats", "-loglevel", "info"]);

    // Encoded input
    if args.start > 0 {
        cmd.args(["-ss", &args.start.to_string()]);
    }
    cmd.args(["-t", &duration.to_string()]);
    cmd.args([
        "-hwaccel",
        "cuda",
        "-hwaccel_output_format",
        "cuda",
        "-i",
        args.encoded.to_str().unwrap(),
    ]);

    // Original input
    if args.start > 0 {
        cmd.args(["-ss", &args.start.to_string()]);
    }
    cmd.args(["-t", &duration.to_string()]);
    cmd.args([
        "-hwaccel",
        "cuda",
        "-hwaccel_output_format",
        "cuda",
        "-i",
        args.original.to_str().unwrap(),
    ]);

    cmd.args(["-lavfi", &filter_chain, "-f", "null", "-"]);

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
        let last_lines: String = output_accumulator
            .lines()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");

        return Err(VideoError::VmafError(format!(
            "FFmpeg failed with exit code {}. Recent output:\n{}",
            status.code().unwrap_or(-1),
            last_lines
        )));
    }

    let scores = parse_vmaf_output(&output_accumulator)?;
    Ok(scores)
}

fn parse_vmaf_output(output: &str) -> Result<VmafScores> {
    for line in output.lines() {
        if let Some(caps) = VMAF_SCORE_RE.captures(line) {
            // Check for simple aggregate score: VMAF score: 95.123
            if let Some(score_match) = caps.get(1) {
                return Ok(VmafScores {
                    mean: score_match.as_str().parse().unwrap_or(0.0),
                    min: None,
                    max: None,
                });
            }

            // Check for detailed summary: mean: 95.123 min: 80.456 max: 99.123
            if let (Some(mean), Some(min), Some(max)) = (caps.get(2), caps.get(3), caps.get(4)) {
                return Ok(VmafScores {
                    mean: mean.as_str().parse().unwrap_or(0.0),
                    min: Some(min.as_str().parse().unwrap_or(0.0)),
                    max: Some(max.as_str().parse().unwrap_or(0.0)),
                });
            }
        }
    }

    let log_path = std::env::temp_dir().join("media-forge-vmaf-error.log");
    let _ = std::fs::write(&log_path, output);
    Err(VideoError::VmafError(format!(
        "Could not parse VMAF scores from output. Full log saved to: {}",
        log_path.display()
    )))
}

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

    println!("\n{}", "=".repeat(50));
    println!("VMAF Results:");
    print!("  Mean Score: {}{:.2}\x1b[0m", color, scores.mean);
    println!(" ({})", rating);

    if let Some(min) = scores.min {
        println!("  Min Score:  {:.2}", min);
    }
    if let Some(max) = scores.max {
        println!("  Max Score:  {:.2}", max);
    }
    println!("{}\n", "=".repeat(50));
}
