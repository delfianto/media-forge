use media_forge::image::{ArchiveArgs, run_archive};
use std::fs::File;
use tempfile::tempdir;
use zip::ZipArchive;

#[test]
fn test_archive_creation_integration() {
    let source_root = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let folder_name = "test_manga";
    let folder_path = source_root.path().join(folder_name);
    std::fs::create_dir(&folder_path).unwrap();

    for i in 0..3 {
        let img_path = folder_path.join(format!("{:03}.jpg", i));
        let img = image::RgbImage::new(10, 10);
        img.save(&img_path).unwrap();
    }

    let args = ArchiveArgs {
        destination: Some(dest_dir.path().to_path_buf()),
        source: source_root.path().to_path_buf(),
        jobs: Some(1),
        recursive: true,
        cleanup: false,
        dry_run: false,
        force: false,
    };

    run_archive(args).unwrap();

    let cbz_path = dest_dir.path().join(format!("{}.cbz", folder_name));
    assert!(cbz_path.exists());

    let file = File::open(&cbz_path).unwrap();
    let archive = ZipArchive::new(file).unwrap();

    assert_eq!(archive.len(), 3);
}
