use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Supported hardware acceleration methods for FFmpeg.
pub enum HwAccel {
    /// NVIDIA CUDA hardware acceleration.
    Cuda,
}

/// Supported video codecs for encoding.
pub enum VideoCodec {
    /// AV1 encoding via NVIDIA NVENC.
    Av1Nvenc,
}

/// Builder for constructing FFmpeg commands with a fluent API.
pub struct FfmpegCommandBuilder {
    input: PathBuf,
    output: PathBuf,
    hwaccel: Option<HwAccel>,
    video_codec: Option<VideoCodec>,
    preset: Option<String>,
    cq: Option<u8>,
    extra_args: Vec<String>,
    start_time: Option<u64>,
    duration: Option<f64>,
}

impl FfmpegCommandBuilder {
    /// Creates a new builder with the specified input and output paths.
    pub fn new(input: &Path, output: &Path) -> Self {
        Self {
            input: input.to_path_buf(),
            output: output.to_path_buf(),
            hwaccel: None,
            video_codec: None,
            preset: None,
            cq: None,
            extra_args: Vec::new(),
            start_time: None,
            duration: None,
        }
    }

    /// Sets the hardware acceleration method.
    pub fn hwaccel(mut self, hwaccel: HwAccel) -> Self {
        self.hwaccel = Some(hwaccel);
        self
    }

    /// Sets the video codec for encoding.
    pub fn video_codec(mut self, codec: VideoCodec) -> Self {
        self.video_codec = Some(codec);
        self
    }

    /// Sets the encoding preset.
    pub fn preset(mut self, preset: &str) -> Self {
        self.preset = Some(preset.to_string());
        self
    }

    /// Sets the constant quality (CQ) level.
    pub fn cq(mut self, cq: u8) -> Self {
        self.cq = Some(cq);
        self
    }

    /// Sets the start time for processing.
    pub fn start_time(mut self, seconds: u64) -> Self {
        self.start_time = Some(seconds);
        self
    }

    /// Sets the duration for processing.
    pub fn duration(mut self, seconds: f64) -> Self {
        self.duration = Some(seconds);
        self
    }

    /// Adds extra arguments to the FFmpeg command.
    pub fn arg(mut self, arg: &str) -> Self {
        self.extra_args.push(arg.to_string());
        self
    }

    /// Adds multiple extra arguments to the FFmpeg command.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.extra_args.push(arg.as_ref().to_string());
        }
        self
    }

    /// Builds and returns the configured FFmpeg `Command`.
    pub fn build(self) -> Command {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-stats");

        if let Some(start) = self.start_time {
            cmd.arg("-ss").arg(start.to_string());
        }

        if let Some(dur) = self.duration {
            cmd.arg("-t").arg(dur.to_string());
        }

        if let Some(hw) = &self.hwaccel {
            match hw {
                HwAccel::Cuda => {
                    cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
                }
            }
        }

        cmd.arg("-i").arg(&self.input);

        if let Some(codec) = &self.video_codec {
            match codec {
                VideoCodec::Av1Nvenc => {
                    cmd.args(["-c:v", "av1_nvenc"]);
                }
            }
        }

        if let Some(p) = &self.preset {
            cmd.arg("-preset").arg(p);
        }

        if let Some(q) = self.cq {
            cmd.arg("-cq").arg(q.to_string());
        }

        for arg in self.extra_args {
            cmd.arg(arg);
        }

        cmd.arg(&self.output);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let builder = FfmpegCommandBuilder::new(Path::new("in.mp4"), Path::new("out.mkv"))
            .hwaccel(HwAccel::Cuda)
            .video_codec(VideoCodec::Av1Nvenc)
            .preset("p6")
            .cq(28);

        let cmd = builder.build();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();

        assert!(args.contains(&"-y"));
        assert!(args.contains(&"cuda"));
        assert!(args.contains(&"av1_nvenc"));
        assert!(args.contains(&"p6"));
        assert!(args.contains(&"28"));
        assert_eq!(cmd.get_program(), "ffmpeg");
    }

    #[test]
    fn test_builder_time_and_duration() {
        let cmd = FfmpegCommandBuilder::new(Path::new("in.mp4"), Path::new("out.mkv"))
            .start_time(10)
            .duration(30.5)
            .build();

        let args: Vec<_> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();
        let ss_idx = args.iter().position(|&r| r == "-ss").unwrap();
        assert_eq!(args[ss_idx + 1], "10");
        let t_idx = args.iter().position(|&r| r == "-t").unwrap();
        assert_eq!(args[t_idx + 1], "30.5");
    }

    #[test]
    fn test_builder_extra_args() {
        let cmd = FfmpegCommandBuilder::new(Path::new("in.mp4"), Path::new("out.mkv"))
            .arg("-custom")
            .args(["-a", "b"])
            .build();

        let args: Vec<_> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();
        assert!(args.contains(&"-custom"));
        assert!(args.contains(&"-a"));
        assert!(args.contains(&"b"));
    }
}
