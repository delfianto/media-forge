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
///
/// These formats can be loaded and converted to AVIF or WebP:
/// - **JPG/JPEG**: Joint Photographic Experts Group (lossy)
/// - **PNG**: Portable Network Graphics (lossless)
/// - **WebP**: Google's modern image format
/// - **TIFF**: Tagged Image File Format (archival)
/// - **BMP**: Windows Bitmap (uncompressed)
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "tiff", "bmp"];

/// Supported video file extensions for encoding.
///
/// These container formats are supported for AV1 encoding:
/// - **MP4**: MPEG-4 Part 14 (most common)
/// - **MKV**: Matroska Video (flexible, open format)
/// - **MOV**: QuickTime File Format (Apple)
/// - **AVI**: Audio Video Interleave (legacy)
/// - **TS**: MPEG Transport Stream (broadcast)
/// - **M4V**: iTunes Video (Apple DRM variant of MP4)
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "ts", "m4v"];

/// Supported archive file extensions.
///
/// Archives containing images can be processed directly:
/// - **ZIP**: Standard ZIP archive
/// - **CBZ**: Comic Book ZIP (ZIP with .cbz extension)
pub const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "cbz"];

/// Recursive file scanner with configurable depth.
///
/// Scans directories for files matching supported media extensions
/// (images, videos, and archives). Uses [`WalkDir`] internally for
/// efficient traversal with configurable recursion depth.
///
/// # Example
///
/// ```no_run
/// use mf_core::Scanner;
/// use std::path::Path;
///
/// // Scan up to 3 levels deep
/// let scanner = Scanner::new(3);
/// let files = scanner.scan(Path::new("./media"));
///
/// for file in files {
///     println!("Found: {}", file.display());
/// }
/// ```
pub struct Scanner {
    /// Maximum recursion depth for directory traversal.
    /// A depth of 1 scans only the root directory.
    /// A depth of 2 includes immediate subdirectories, etc.
    pub max_depth: usize,
}

impl Scanner {
    /// Creates a new scanner with the specified maximum depth.
    ///
    /// # Arguments
    ///
    /// * `max_depth` - Maximum number of directory levels to traverse.
    ///   A value of 1 scans only the root, 2 includes subdirectories, etc.
    ///
    /// # Example
    ///
    /// ```
    /// use mf_core::Scanner;
    ///
    /// let scanner = Scanner::new(5); // Scan up to 5 levels deep
    /// ```
    pub fn new(max_depth: usize) -> Self {
        Self { max_depth }
    }

    /// Scans a directory for supported media files.
    ///
    /// Recursively walks the directory tree up to `max_depth` levels,
    /// collecting all files with extensions matching [`IMAGE_EXTENSIONS`],
    /// [`VIDEO_EXTENSIONS`], or [`ARCHIVE_EXTENSIONS`].
    ///
    /// # Arguments
    ///
    /// * `root` - The root directory to start scanning from.
    ///
    /// # Returns
    ///
    /// A vector of [`PathBuf`] containing absolute paths to all matching files.
    /// Files are not guaranteed to be in any particular order.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mf_core::Scanner;
    /// use std::path::Path;
    ///
    /// let scanner = Scanner::new(2);
    /// let files = scanner.scan(Path::new("/home/user/photos"));
    /// println!("Found {} media files", files.len());
    /// ```
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
///
/// Provides intelligent defaults for parallel processing based on
/// available CPU cores, with configurable overrides and safety limits.
///
/// # Thread Count Strategy
///
/// - **Default**: 75% of available cores (leaves headroom for system)
/// - **Maximum**: 150% of available cores (prevents over-subscription)
/// - **Minimum**: 1 thread (always functional)
///
/// # Example
///
/// ```
/// use mf_core::CpuControl;
///
/// // Use default (75% of cores)
/// let threads = CpuControl::get_thread_count(None);
///
/// // Request specific count (will be clamped to safe range)
/// let threads = CpuControl::get_thread_count(Some(16));
/// ```
pub struct CpuControl;

impl CpuControl {
    /// Calculates the optimal thread count for parallel processing.
    ///
    /// # Arguments
    ///
    /// * `requested` - Optional user-specified thread count.
    ///   - `None`: Returns 75% of available CPU cores.
    ///   - `Some(n)`: Returns `n` clamped to the range [1, 150% of cores].
    ///
    /// # Returns
    ///
    /// The number of threads to use, guaranteed to be at least 1.
    ///
    /// # Example
    ///
    /// ```
    /// use mf_core::CpuControl;
    ///
    /// // On an 8-core system:
    /// assert!(CpuControl::get_thread_count(None) >= 1);
    ///
    /// // User requests are clamped to safe range
    /// let threads = CpuControl::get_thread_count(Some(1000));
    /// assert!(threads <= num_cpus::get() * 2);
    /// ```
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
///
/// Provides helper functions for working with filenames in the context
/// of media processing and comic book archive creation.
pub struct Naming;

impl Naming {
    /// Truncates a filename to fit within a maximum length.
    ///
    /// If the filename exceeds `max_len`, it is truncated and "..." is
    /// appended to indicate truncation. Useful for display in progress
    /// bars or fixed-width terminal output.
    ///
    /// # Arguments
    ///
    /// * `name` - The filename to potentially truncate.
    /// * `max_len` - Maximum allowed length including the "..." suffix.
    ///
    /// # Returns
    ///
    /// The original filename if it fits, or a truncated version with "...".
    ///
    /// # Example
    ///
    /// ```
    /// use mf_core::Naming;
    ///
    /// assert_eq!(Naming::truncate_filename("short.txt", 20), "short.txt");
    /// assert_eq!(Naming::truncate_filename("very_long_filename.txt", 10), "very_lo...");
    /// ```
    ///
    /// # Note
    ///
    /// This function operates on bytes, not Unicode characters. For filenames
    /// with multi-byte UTF-8 characters, the truncation point may not align
    /// with character boundaries.
    pub fn truncate_filename(name: &str, max_len: usize) -> String {
        if name.len() <= max_len {
            name.to_string()
        } else {
            let truncated = &name[..max_len - 3];
            format!("{}...", truncated)
        }
    }

    /// Determines if a filename represents a cover image.
    ///
    /// Cover images are typically the first image in a comic book archive
    /// and are used for thumbnails and previews in comic readers.
    ///
    /// # Detection Rules
    ///
    /// A filename is considered a cover image if (case-insensitive):
    /// - Contains "000_cover" (explicit cover marker)
    /// - Starts with "000" (common first-page convention)
    /// - Contains "cover" anywhere in the name
    ///
    /// # Arguments
    ///
    /// * `filename` - The filename to check (without path).
    ///
    /// # Returns
    ///
    /// `true` if the filename matches cover image patterns.
    ///
    /// # Example
    ///
    /// ```
    /// use mf_core::Naming;
    ///
    /// assert!(Naming::is_cover_image("000_cover.jpg"));
    /// assert!(Naming::is_cover_image("000.png"));
    /// assert!(Naming::is_cover_image("front_cover.webp"));
    /// assert!(!Naming::is_cover_image("page_001.jpg"));
    /// ```
    pub fn is_cover_image(filename: &str) -> bool {
        let lower = filename.to_lowercase();
        lower.contains("000_cover") || lower.starts_with("000") || lower.contains("cover")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_filename() {
        assert_eq!(Naming::truncate_filename("short.txt", 20), "short.txt");
    }

    #[test]
    fn test_truncate_long_filename() {
        let result = Naming::truncate_filename("very_long_filename.txt", 10);
        assert_eq!(result, "very_lo...");
        assert!(result.len() <= 10);
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(Naming::truncate_filename("exact.txt", 9), "exact.txt");
    }

    #[test]
    fn test_is_cover_image_explicit() {
        assert!(Naming::is_cover_image("000_cover.jpg"));
        assert!(Naming::is_cover_image("000_COVER.PNG"));
    }

    #[test]
    fn test_is_cover_image_prefix() {
        assert!(Naming::is_cover_image("000.png"));
        assert!(Naming::is_cover_image("000_first_page.jpg"));
    }

    #[test]
    fn test_is_cover_image_contains() {
        assert!(Naming::is_cover_image("front_cover.webp"));
        assert!(Naming::is_cover_image("COVER_IMAGE.jpg"));
    }

    #[test]
    fn test_is_not_cover_image() {
        assert!(!Naming::is_cover_image("page_001.jpg"));
        assert!(!Naming::is_cover_image("001.png"));
        assert!(!Naming::is_cover_image("chapter_1.webp"));
    }

    #[test]
    fn test_cpu_control_default() {
        let count = CpuControl::get_thread_count(None);
        assert!(count >= 1);
        let total = num_cpus::get();
        assert!(count <= (total as f64 * 1.5).ceil() as usize);
    }

    #[test]
    fn test_cpu_control_requested() {
        let count = CpuControl::get_thread_count(Some(4));
        assert!(count >= 1);
    }

    #[test]
    fn test_cpu_control_clamped_high() {
        let count = CpuControl::get_thread_count(Some(10000));
        let total = num_cpus::get();
        assert!(count <= (total as f64 * 1.5).ceil() as usize);
    }

    #[test]
    fn test_cpu_control_clamped_low() {
        let count = CpuControl::get_thread_count(Some(0));
        assert_eq!(count, 1);
    }

    #[test]
    fn test_scanner_new() {
        let scanner = Scanner::new(5);
        assert_eq!(scanner.max_depth, 5);
    }
}
