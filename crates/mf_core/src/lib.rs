use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "tiff", "bmp"];
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "ts", "m4v"];
pub const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "cbz"];

pub struct Scanner {
    pub max_depth: usize,
}

impl Scanner {
    pub fn new(max_depth: usize) -> Self {
        Self { max_depth }
    }

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

pub struct CpuControl;

impl CpuControl {
    pub fn get_thread_count(requested: Option<usize>) -> usize {
        let total_cpus = num_cpus::get();
        let default_threads = total_cpus / 2;
        let max_safe = (total_cpus as f64 * 0.75).ceil() as usize;

        let threads = requested.unwrap_or(default_threads);
        threads.clamp(1, max_safe)
    }
}

pub struct Naming;

impl Naming {
    pub fn truncate_filename(name: &str, max_len: usize) -> String {
        if name.len() <= max_len {
            name.to_string()
        } else {
            let truncated = &name[..max_len - 3];
            format!("{}...", truncated)
        }
    }

    pub fn is_cover_image(filename: &str) -> bool {
        let lower = filename.to_lowercase();
        lower.contains("000_cover") || lower.starts_with("000") || lower.contains("cover")
    }
}
