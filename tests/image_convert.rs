use image::GenericImageView;
use media_forge::image::{ImageArgs, run};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_image_conversion_integration() {
    let source_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let img_path = source_dir.path().join("test.jpg");
    let img = image::RgbImage::new(100, 100);
    img.save(&img_path).unwrap();

    let args = ImageArgs {
        destination: dest_dir.path().to_path_buf(),
        source: vec![source_dir.path().to_path_buf()],
        format: "avif".to_string(),
        quality: 50,
        speed: 8,
        depth: 1,
        jobs: Some(1),
        no_mtime: true,
        overwrite: false,
        report: false,
    };

    run(args).unwrap();

    let output_path = dest_dir.path().join("test.avif");
    assert!(output_path.exists());

    let out_img = image::open(&output_path).unwrap();
    assert_eq!(out_img.dimensions(), (100, 100));
}

#[test]
fn test_image_conversion_preserve_mtime() {
    let source_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let img_path = source_dir.path().join("mtime_test.jpg");
    let img = image::RgbImage::new(10, 10);
    img.save(&img_path).unwrap();

    let original_mtime = fs::metadata(&img_path).unwrap().modified().unwrap();

    let args = ImageArgs {
        destination: dest_dir.path().to_path_buf(),
        source: vec![source_dir.path().to_path_buf()],
        format: "webp".to_string(),
        quality: 50,
        speed: 8,
        depth: 1,
        jobs: Some(1),
        no_mtime: false,
        overwrite: false,
        report: false,
    };

    run(args).unwrap();

    let output_path = dest_dir.path().join("mtime_test.webp");
    let new_mtime = fs::metadata(&output_path).unwrap().modified().unwrap();

    // Check if mtimes are very close (preservation might have slight precision loss on some filesystems
    // but usually it's exact in Rust's set_modified)
    assert_eq!(original_mtime, new_mtime);
}
