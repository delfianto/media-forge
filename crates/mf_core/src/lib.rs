//! Core utilities for the media-forge project.
//!
//! This crate provides shared functionality used across all media-forge modules:
//! - File scanning with configurable depth and extension filtering
//! - CPU thread count management with sensible defaults
//! - Filename utilities for display and cover image detection
//!
//! # Example
//!
//! ```no_run
//! use mf_core::{Scanner, CpuControl, IMAGE_EXTENSIONS};
//! use std::path::Path;
//!
//! // Scan for media files
//! let scanner = Scanner::new(3);
//! let files = scanner.scan(Path::new("./media"));
//!
//! // Get optimal thread count
//! let threads = CpuControl::get_thread_count(None);
//! ```

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Supported image file extensions for conversion.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "tiff", "bmp"];

/// Supported video file extensions for encoding.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "ts", "m4v"];

/// Supported archive file extensions.
pub const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "cbz"];

/// Recursive file scanner with configurable depth.
pub struct Scanner {
    /// Maximum recursion depth for directory traversal.
    pub max_depth: usize,
}

impl Scanner {
    /// Creates a new scanner with the specified maximum depth.
    pub fn new(max_depth: usize) -> Self {
        Self { max_depth }
    }

    /// Scans a directory for supported media files.
    pub fn scan(&self, root: &Path) -> Vec<PathBuf> {
        WalkDir::new(root)
            .max_depth(self.max_depth)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.path().to_path_buf())
            .filter(|p| {
                let ext = p
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_lowercase());
                if let Some(ext) = ext {
                    IMAGE_EXTENSIONS.contains(&ext.as_str())
                        || ARCHIVE_EXTENSIONS.contains(&ext.as_str())
                        || VIDEO_EXTENSIONS.contains(&ext.as_str())
                } else {
                    false
                }
            })
            .collect()
    }
}

/// CPU thread count management utilities.
pub struct CpuControl;

impl CpuControl {
    /// Calculates the optimal thread count for parallel processing.
    pub fn get_thread_count(requested: Option<usize>) -> usize {
        let total_cpus = num_cpus::get();
        let default_threads = (total_cpus as f64 * 0.75).ceil() as usize;
        let max_limit = (total_cpus as f64 * 1.5).ceil() as usize;

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

    /// Determines if a filename represents a cover image using heuristics.
    pub fn is_cover_image(filename: &str) -> bool {
        let lower = filename.to_lowercase();

        // Explicit markers
        if lower.contains("cover") || lower.contains("folder") || lower.contains("front") {
            return true;
        }

        // Common first page patterns
        if lower.starts_with("000") || lower.starts_with("001") || lower.starts_with("page_000") {
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_unicode() {
        // "🦀" is one character but 4 bytes
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
    fn test_is_cover_image() {
        assert!(Naming::is_cover_image("000_cover.jpg"));
        assert!(Naming::is_cover_image("000.png"));
        assert!(Naming::is_cover_image("front_cover.webp"));
        assert!(Naming::is_cover_image("Folder.jpg"));
        assert!(!Naming::is_cover_image("page_005.jpg"));
    }
}
