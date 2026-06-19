//! Core utilities for the media-forge project.
//!
//! This crate provides shared functionality used across all media-forge modules:
//! - File scanning with configurable depth and extension filtering
//! - CPU thread count management with sensible defaults
//! - Filename utilities for display and cover image detection

pub mod constants;
pub mod image;
pub mod ui;
pub mod video;
pub mod walker;

use crate::constants::{DEFAULT_CPU_RATIO, MAX_CPU_RATIO};
use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Global flag to signal shutdown (e.g., on Ctrl+C).
pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Supported image extensions for conversion (excludes already-optimal formats).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tiff", "bmp"];

/// Supported archive extensions for image extraction.
pub const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "cbz"];

/// Supported video extensions for hardware-accelerated encoding.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "ts", "m4v", "mpv", "webm"];

/// Image extensions that are passed through without conversion.
/// Includes already-optimal modern formats (avif, webp) and animated formats (gif, apng)
/// that the image pipeline cannot re-encode meaningfully.
pub const PASSTHROUGH_IMAGE_EXTENSIONS: &[&str] = &[
    "avif", // already an optimal format — re-encoding would only degrade quality
    "webp", // already an optimal format — re-encoding would only degrade quality
    "gif",  // animated — cannot be converted to static avif/webp without losing animation
    "apng", // animated PNG — cannot be converted to static avif/webp without losing animation
];

/// Registry for active child process PIDs to ensure cleanup on termination.
static ACTIVE_PROCESSES: Lazy<Mutex<HashSet<u32>>> = Lazy::new(|| Mutex::new(HashSet::new()));

/// Manages external child processes to ensure they are cleaned up on exit.
pub struct ProcessManager;

impl ProcessManager {
    /// Registers a child process PID for tracking.
    pub fn register(pid: u32) {
        if let Ok(mut pids) = ACTIVE_PROCESSES.lock() {
            pids.insert(pid);
        }
    }

    /// Unregisters a child process PID once it has completed.
    pub fn unregister(pid: u32) {
        if let Ok(mut pids) = ACTIVE_PROCESSES.lock() {
            pids.remove(&pid);
        }
    }

    /// Kills all registered child processes and signals shutdown.
    pub fn kill_all() {
        SHUTDOWN.store(true, Ordering::SeqCst);
        if let Ok(mut pids) = ACTIVE_PROCESSES.lock() {
            for &pid in pids.iter() {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .status();
            }
            pids.clear();
        }
    }
}

/// Path utilities for source and destination management.
pub struct PathUtil;

impl PathUtil {
    /// Canonicalizes the source path and resolves the destination path relative to CWD if needed.
    pub fn resolve_paths(
        source: &Path,
        destination: &Path,
    ) -> Result<(PathBuf, PathBuf), anyhow::Error> {
        let source_path = std::fs::canonicalize(source)?;

        let dest_path = if destination.is_absolute() {
            destination.to_path_buf()
        } else {
            std::env::current_dir()?.join(destination)
        };

        Ok((source_path, dest_path))
    }

    /// Checks if a file should be skipped based on existence and the overwrite flag.
    pub fn should_skip(path: &Path, overwrite: bool) -> bool {
        if overwrite {
            return false;
        }
        if let Ok(metadata) = std::fs::metadata(path) {
            return metadata.len() > 0;
        }
        false
    }

    /// Recursively calculates the size of a directory or file.
    pub fn get_dir_size(path: &Path) -> u64 {
        if path.is_file() {
            return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        }

        use walkdir::WalkDir;
        WalkDir::new(path)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .map(|m| m.len())
            .sum()
    }
}

/// Formats a size in bytes to a human-readable string (e.g., "1.5 GB").
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// CPU thread count management utilities.
pub struct CpuControl;

impl CpuControl {
    /// Calculates the optimal thread count for parallel processing.
    ///
    /// Defaults to a percentage of available cores if no specific count is requested.
    /// Clamps the result between 1 and a maximum percentage of available cores.
    pub fn get_thread_count(requested: Option<usize>) -> usize {
        let total_cpus = num_cpus::get();
        let default_threads = (total_cpus as f64 * DEFAULT_CPU_RATIO).ceil() as usize;
        let max_limit = (total_cpus as f64 * MAX_CPU_RATIO).ceil() as usize;

        match requested {
            Some(req) => req.clamp(1, max_limit),
            None => default_threads.clamp(1, max_limit),
        }
    }
}

/// Filename utilities for display and cover image detection.
pub struct Naming;

impl Naming {
    /// Truncates a filename to fit within a maximum length, handling Unicode correctly.
    pub fn truncate_filename(name: &str, max_len: usize) -> String {
        let char_count = name.chars().count();
        if char_count <= max_len {
            name.to_string()
        } else {
            let truncated: String = name.chars().take(max_len.saturating_sub(3)).collect();
            format!("{}...", truncated)
        }
    }

    /// Truncates a filename to fit within a maximum length by keeping the end of the name.
    pub fn truncate_from_start(name: &str, max_len: usize) -> String {
        let char_count = name.chars().count();
        if char_count <= max_len {
            name.to_string()
        } else {
            let keep_len = max_len.saturating_sub(3);
            let suffix: String = name.chars().rev().take(keep_len).collect();
            let reversed: String = suffix.chars().rev().collect();
            format!("...{}", reversed)
        }
    }

    /// Determines if a filename represents a cover image using heuristics.
    pub fn is_cover_image(filename: &str) -> bool {
        let lower = filename.to_lowercase();

        if lower.contains("cover") || lower.contains("folder") || lower.contains("front") {
            return true;
        }

        if lower.starts_with("000") || lower.starts_with("001") || lower.starts_with("page_000") {
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_truncate_unicode() {
        assert_eq!(Naming::truncate_filename("🦀🦀🦀🦀🦀", 4), "🦀...");
    }

    #[test]
    fn test_truncate_short_filename() {
        assert_eq!(Naming::truncate_filename("short.txt", 20), "short.txt");
    }

    #[test]
    fn test_truncate_long_filename() {
        let result = Naming::truncate_filename("very_long_filename.txt", 10);
        assert_eq!(result, "very_lo...");
        assert_eq!(result.chars().count(), 10);
    }

    #[test]
    fn test_truncate_from_start() {
        assert_eq!(Naming::truncate_from_start("🦀🦀🦀🦀🦀", 4), "...🦀");
        assert_eq!(
            Naming::truncate_from_start("long_filename.txt", 10),
            "...ame.txt"
        );
    }

    #[test]
    fn test_is_cover_image() {
        assert!(Naming::is_cover_image("000_cover.jpg"));
        assert!(Naming::is_cover_image("000.png"));
        assert!(Naming::is_cover_image("front_cover.webp"));
        assert!(Naming::is_cover_image("Folder.jpg"));
        assert!(!Naming::is_cover_image("page_005.jpg"));
    }

    #[test]
    fn test_cpu_control_default() {
        let count = CpuControl::get_thread_count(None);
        assert!(count >= 1);
        assert!(count <= (num_cpus::get() as f64 * MAX_CPU_RATIO).ceil() as usize);
    }

    #[test]
    fn test_cpu_control_clamped() {
        let count = CpuControl::get_thread_count(Some(9999));
        let max = (num_cpus::get() as f64 * MAX_CPU_RATIO).ceil() as usize;
        assert_eq!(count, max);

        let count = CpuControl::get_thread_count(Some(0));
        assert_eq!(count, 1);
    }

    #[test]
    fn test_path_util_should_skip() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        assert!(!PathUtil::should_skip(&file_path, false));

        std::fs::write(&file_path, "content").unwrap();
        assert!(PathUtil::should_skip(&file_path, false));
        assert!(!PathUtil::should_skip(&file_path, true));

        let empty_file = dir.path().join("empty.txt");
        std::fs::write(&empty_file, "").unwrap();
        assert!(!PathUtil::should_skip(&empty_file, false));
    }
}
