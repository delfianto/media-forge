use crate::image::{ImageError, QualityArgs, load_image};
use image::GenericImageView;
use ssimulacra2::{ColorPrimaries, Rgb, TransferCharacteristic, compute_frame_ssimulacra2};
use std::path::Path;

/// Executes the image quality analysis process.
///
/// This function loads the original and distorted images from the specified paths,
/// computes the SSIMULACRA2 score, and displays the results.
pub fn run(args: QualityArgs) -> anyhow::Result<()> {
    println!("Comparing images using SSIMULACRA2...");
    println!("Original:  {}", args.original.display());
    println!("Distorted: {}
", args.distorted.display());

    let score = compute_quality(&args.original, &args.distorted)?;
    display_results(score);

    Ok(())
}

/// Computes the SSIMULACRA2 score between two images loaded from paths.
///
/// Returns the quality score (0-100) or an error if images cannot be loaded or dimensions mismatch.
pub fn compute_quality(original: &Path, distorted: &Path) -> anyhow::Result<f64> {
    let img1 = load_image(original)?;
    let img2 = load_image(distorted)?;
    compute_quality_from_image(&img1, &img2)
}

/// Computes the SSIMULACRA2 score between two in-memory images.
///
/// Requires both images to have the same dimensions.
pub fn compute_quality_from_image(
    img1: &image::DynamicImage,
    img2: &image::DynamicImage,
) -> anyhow::Result<f64> {
    if img1.dimensions() != img2.dimensions() {
        return Err(ImageError::DimensionMismatch(img1.dimensions(), img2.dimensions()).into());
    }

    let (width, height) = img1.dimensions();

    let img1_data = img1
        .to_rgb32f()
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect::<Vec<_>>();
    let img1_rgb = Rgb::new(
        img1_data,
        width as usize,
        height as usize,
        TransferCharacteristic::SRGB,
        ColorPrimaries::BT709,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create RGB frame for original: {:?}", e))?;

    let img2_data = img2
        .to_rgb32f()
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect::<Vec<_>>();
    let img2_rgb = Rgb::new(
        img2_data,
        width as usize,
        height as usize,
        TransferCharacteristic::SRGB,
        ColorPrimaries::BT709,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create RGB frame for distorted: {:?}", e))?;

    let score = compute_frame_ssimulacra2(img1_rgb, img2_rgb)
        .map_err(|e| anyhow::anyhow!("SSIMULACRA2 calculation failed: {}", e))?;

    Ok(score)
}

/// Converts a numerical SSIMULACRA2 score into a human-readable rating.
pub fn get_rating(score: f64) -> String {
    if score >= 90.0 {
        "Excellent".to_string()
    } else if score >= 70.0 {
        "Very Good".to_string()
    } else if score >= 50.0 {
        "Good".to_string()
    } else if score >= 30.0 {
        "Fair".to_string()
    } else {
        "Poor".to_string()
    }
}

/// Prints the SSIMULACRA2 score and its corresponding rating to the console.
fn display_results(score: f64) {
    let rating = get_rating(score);
    let color = match rating.as_str() {
        "Excellent" | "Very Good" => "\x1b[92m",
        "Good" | "Fair" => "\x1b[93m",
        _ => "\x1b[91m",
    };

    println!("{}", "=".repeat(50));
    println!("SSIMULACRA2 Results:");
    println!("  Score: {}{:.2}\x1b[0m ({})", color, score, rating);
    println!("{}\n", "=".repeat(50));
}