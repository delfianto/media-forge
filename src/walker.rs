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
    pub fn scan_flat(&self, root: &Path) -> Vec<Asset> {
        let mut assets = Vec::new();
        let mut _containers = 0;
        self.scan_recursive(root, &mut assets, &mut _containers, 0, &|_, _| {});
        assets
    }

    pub fn scan_with_progress(&self, root: &Path, msg: &str) -> Vec<Asset> {
        let pb = crate::ui::create_scanner(msg);
        let mut assets = Vec::new();
        let mut containers = 0;

        self.scan_recursive(root, &mut assets, &mut containers, 0, &|c, i| {
            pb.set_message(format!(
                "Scanning... {} containers and {} items found",
                c, i
            ));
        });

        pb.finish_and_clear();
        assets
    }

    fn scan_recursive(
        &self,
        current_path: &Path,
        assets: &mut Vec<Asset>,
        containers: &mut usize,
        depth: usize,
        cb: &dyn Fn(usize, usize),
    ) {
        if depth > self.max_depth {
            return;
        }

        if current_path.is_file() {
            let (added, is_archive) = self.check_and_add_file(current_path, assets);
            if added {
                if is_archive {
                    *containers += 1;
                }
                cb(*containers, assets.len());
            }
            return;
        }

        if !current_path.is_dir() {
            return;
        }

        // Folders are containers too
        *containers += 1;
        cb(*containers, assets.len());

        let entries: Vec<_> = WalkDir::new(current_path)
            .max_depth(1)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path() != current_path)
            .collect();

        for entry in entries {
            let path = entry.path();
            if path.is_file() {
                let (added, is_archive) = self.check_and_add_file(path, assets);
                if added {
                    if is_archive {
                        *containers += 1;
                    }
                    cb(*containers, assets.len());
                }
            } else if path.is_dir() && self.recursive {
                self.scan_recursive(path, assets, containers, depth + 1, cb);
            }
        }
    }

    /// Checks if a file matches the target extensions and adds it to the assets list.
    /// Returns (added_any, is_archive).
    fn check_and_add_file(&self, path: &Path, assets: &mut Vec<Asset>) -> (bool, bool) {
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
            return (true, false);
        }

        if ext == "zip" || ext == "cbz" {
            let added = self.scan_archive(path, assets);
            return (added, true);
        }
        (false, false)
    }

    /// Inspects a supported archive (zip/cbz) and adds its contents as assets.
    fn scan_archive(&self, path: &Path, assets: &mut Vec<Asset>) -> bool {
        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };

        let mut archive = match ZipArchive::new(file) {
            Ok(a) => a,
            Err(_) => return false,
        };

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
        added
    }

    /// Scans and groups assets by their parent directory.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    #[test]
    fn test_walker_scan_flat() {
        let dir = tempdir().expect("Failed to create temp dir");
        let root = dir.path();

        File::create(root.join("test1.jpg")).expect("Failed to create test file");
        File::create(root.join("test2.png")).expect("Failed to create test file");
        File::create(root.join("other.txt")).expect("Failed to create test file");

        let walker = Walker::new(&["jpg", "png"], 1, false);
        let assets = walker.scan_flat(root);

        assert_eq!(assets.len(), 2);
    }

    #[test]
    fn test_walker_recursive() {
        let dir = tempdir().expect("Failed to create temp dir");
        let root = dir.path();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).expect("Failed to create sub dir");

        File::create(root.join("root.jpg")).expect("Failed to create test file");
        File::create(sub.join("sub.jpg")).expect("Failed to create test file");

        let walker = Walker::new(&["jpg"], 2, true);
        let assets = walker.scan_flat(root);
        assert_eq!(assets.len(), 2);

        let walker_no_rec = Walker::new(&["jpg"], 2, false);
        let assets_no_rec = walker_no_rec.scan_flat(root);
        assert_eq!(assets_no_rec.len(), 1);
    }

    #[test]
    fn test_walker_archive_inspection() {
        let dir = tempdir().expect("Failed to create temp dir");
        let root = dir.path();
        let zip_path = root.join("test.cbz");

        let file = File::create(&zip_path).expect("Failed to create test file");
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("img1.jpg", options)
            .expect("Failed to start file in zip");
        zip.write_all(b"fake data")
            .expect("Failed to write data to zip");
        zip.start_file("notes.txt", options)
            .expect("Failed to start file in zip");
        zip.write_all(b"text").expect("Failed to write data to zip");
        zip.finish().expect("Failed to finish zip");

        let walker = Walker::new(&["jpg"], 1, false);
        let assets = walker.scan_flat(root);

        assert_eq!(assets.len(), 1);
    }

    #[test]
    fn test_scan_grouped() {
        let dir = tempdir().expect("Failed to create temp dir");
        let root = dir.path();
        let sub1 = root.join("sub1");
        let sub2 = root.join("sub2");
        std::fs::create_dir(&sub1).expect("Failed to create sub dir");
        std::fs::create_dir(&sub2).expect("Failed to create sub dir");

        File::create(sub1.join("a.jpg")).expect("Failed to create test file");
        File::create(sub1.join("b.jpg")).expect("Failed to create test file");
        File::create(sub2.join("c.jpg")).expect("Failed to create test file");

        let walker = Walker::new(&["jpg"], 2, true);
        let groups = walker.scan_grouped(root);

        assert_eq!(groups.len(), 2);
    }
}
