use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::ZipArchive;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSource {
    Filesystem(PathBuf),
    Archive {
        archive_path: PathBuf,
        entry_name: String,
    },
}

#[derive(Debug, Clone)]
pub struct Asset {
    /// The actual file on disk (zip or image)
    pub path: PathBuf,
    /// Metadata about where it came from
    pub source: MediaSource,
}

pub struct Walker {
    pub extensions: Vec<String>,
    pub max_depth: usize,
    pub recursive: bool,
}

impl Walker {
    pub fn new(extensions: &[&str], max_depth: usize, recursive: bool) -> Self {
        Self {
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
            max_depth,
            recursive,
        }
    }

    /// Scans for files and returns a flat list of assets.
    /// This abstracts away whether an image is a file or inside a zip.
    pub fn scan_flat(&self, root: &Path) -> Vec<Asset> {
        let mut assets = Vec::new();
        self.scan_recursive(root, root, &mut assets, 0);
        assets
    }

    pub fn scan_with_progress(&self, root: &Path, msg: &str) -> Vec<Asset> {
        let pb = crate::ui::create_scanner(msg);
        let mut assets = Vec::new();

        self.scan_recursive_with_cb(root, root, &mut assets, 0, &|count| {
            pb.set_position(count as u64);
            pb.set_message(format!("Scanning... {} items", count));
        });

        pb.finish_and_clear();
        assets
    }

    /// Recursively scans directories with a callback for progress updates.
    fn scan_recursive_with_cb(
        &self,
        _current_root: &Path,
        current_path: &Path,
        assets: &mut Vec<Asset>,
        depth: usize,
        cb: &dyn Fn(usize),
    ) {
        if depth > self.max_depth {
            return;
        }

        if current_path.is_file() {
            if self.check_and_add_file(current_path, assets) {
                cb(assets.len());
            }
            return;
        }

        if !current_path.is_dir() {
            return;
        }

        let walker = WalkDir::new(current_path).max_depth(1).follow_links(true);

        for entry in walker.into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path == current_path {
                continue;
            }

            if path.is_file() {
                if self.check_and_add_file(path, assets) {
                    cb(assets.len());
                }
            } else if path.is_dir() && self.recursive {
                self.scan_recursive_with_cb(_current_root, path, assets, depth + 1, cb);
            }
        }
    }

    /// Scans and groups assets by their parent directory.
    /// Useful for archive creation (CBZ).
    pub fn scan_grouped(&self, root: &Path) -> HashMap<PathBuf, Vec<Asset>> {
        let assets = self.scan_flat(root);
        let mut groups: HashMap<PathBuf, Vec<Asset>> = HashMap::new();

        for asset in assets {
            let parent = match &asset.source {
                MediaSource::Filesystem(p) => p.parent().unwrap_or(root).to_path_buf(),
                MediaSource::Archive { archive_path, .. } => {
                    archive_path.parent().unwrap_or(root).to_path_buf()
                }
            };
            groups.entry(parent).or_default().push(asset);
        }
        groups
    }

    /// Recursively scans directories without progress updates.
    fn scan_recursive(
        &self,
        current_root: &Path,
        current_path: &Path,
        assets: &mut Vec<Asset>,
        depth: usize,
    ) {
        self.scan_recursive_with_cb(current_root, current_path, assets, depth, &|_| {});
    }

    /// Checks if a file matches the target extensions and adds it to the assets list.
    /// Also inspects supported archives (zip/cbz) to add their contents as assets.
    fn check_and_add_file(&self, path: &Path, assets: &mut Vec<Asset>) -> bool {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        if self.extensions.contains(&ext) {
            assets.push(Asset {
                path: path.to_path_buf(),
                source: MediaSource::Filesystem(path.to_path_buf()),
            });
            return true;
        }

        if (ext == "zip" || ext == "cbz")
            && let Ok(file) = fs::File::open(path)
            && let Ok(mut archive) = ZipArchive::new(file)
        {
            let mut added = false;
            for i in 0..archive.len() {
                if let Ok(file) = archive.by_index(i)
                    && file.is_file()
                {
                    let name = file.name().to_string();
                    let entry_path = Path::new(&name);
                    let entry_ext = entry_path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();

                    if self.extensions.contains(&entry_ext) {
                        assets.push(Asset {
                            path: path.to_path_buf(),
                            source: MediaSource::Archive {
                                archive_path: path.to_path_buf(),
                                entry_name: name,
                            },
                        });
                        added = true;
                    }
                }
            }
            return added;
        }
        false
    }
}