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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    #[test]
    fn test_walker_scan_flat() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        File::create(root.join("test1.jpg")).unwrap();
        File::create(root.join("test2.png")).unwrap();
        File::create(root.join("other.txt")).unwrap();

        let walker = Walker::new(&["jpg", "png"], 1, false);
        let assets = walker.scan_flat(root);

        assert_eq!(assets.len(), 2);
        let paths: Vec<_> = assets
            .iter()
            .map(|a| a.path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(paths.contains(&"test1.jpg"));
        assert!(paths.contains(&"test2.png"));
    }

    #[test]
    fn test_walker_recursive() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();

        File::create(root.join("root.jpg")).unwrap();
        File::create(sub.join("sub.jpg")).unwrap();

        let walker = Walker::new(&["jpg"], 2, true);
        let assets = walker.scan_flat(root);
        assert_eq!(assets.len(), 2);

        let walker_no_rec = Walker::new(&["jpg"], 2, false);
        let assets_no_rec = walker_no_rec.scan_flat(root);
        assert_eq!(assets_no_rec.len(), 1);
    }

    #[test]
    fn test_walker_archive_inspection() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let zip_path = root.join("test.cbz");

        let file = File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("img1.jpg", options).unwrap();
        zip.write_all(b"fake data").unwrap();
        zip.start_file("notes.txt", options).unwrap();
        zip.write_all(b"text").unwrap();
        zip.finish().unwrap();

        let walker = Walker::new(&["jpg"], 1, false);
        let assets = walker.scan_flat(root);

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].path, zip_path);
        if let MediaSource::Archive {
            archive_path,
            entry_name,
        } = &assets[0].source
        {
            assert_eq!(archive_path, &zip_path);
            assert_eq!(entry_name, "img1.jpg");
        } else {
            panic!("Expected Archive source");
        }
    }

    #[test]
    fn test_scan_grouped() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let sub1 = root.join("sub1");
        let sub2 = root.join("sub2");
        std::fs::create_dir(&sub1).unwrap();
        std::fs::create_dir(&sub2).unwrap();

        File::create(sub1.join("a.jpg")).unwrap();
        File::create(sub1.join("b.jpg")).unwrap();
        File::create(sub2.join("c.jpg")).unwrap();

        let walker = Walker::new(&["jpg"], 2, true);
        let groups = walker.scan_grouped(root);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups.get(&sub1).unwrap().len(), 2);
        assert_eq!(groups.get(&sub2).unwrap().len(), 1);
    }
}
