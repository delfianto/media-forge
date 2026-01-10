use crate::image::quality::{compute_quality_from_image, get_rating};
use crate::{IMAGE_EXTENSIONS, ui};
use crossbeam_channel::{Receiver, Sender, bounded};
use indicatif::{MultiProgress, ProgressBar};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use walkdir::WalkDir;

/// Represents a single record in the conversion report CSV.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReportRecord {
    /// Name of the processed file.
    pub file_name: String,
    /// Size of the original file in bytes.
    pub original_size: u64,
    /// Size of the converted file in bytes.
    pub final_size: u64,
    /// Computed quality score (SSIMULACRA2).
    pub quality_value: f64,
    /// Qualitative description of the quality score.
    pub quality_desc: String,
}

impl ReportRecord {
    /// Helper to create a record from raw components.
    pub fn new(
        file_name: String,
        original_size: u64,
        final_size: u64,
        quality_value: f64,
        quality_desc: String,
    ) -> Self {
        Self {
            file_name,
            original_size,
            final_size,
            quality_value,
            quality_desc,
        }
    }
}

/// Manages the background reporting thread and statistics.
pub struct Reporter {
    tx: Option<Sender<ReportRecord>>,
    handle: Option<JoinHandle<()>>,
    pub path: PathBuf,
}

impl Reporter {
    /// Initializes a new reporter, spawning a dedicated writer thread.
    ///
    /// The writer thread will consume records from the channel and write them to the CSV file
    /// at `path`.
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = bounded::<ReportRecord>(1000);
        let p = path.clone();

        let handle = std::thread::spawn(move || {
            Self::run_writer(rx, p);
        });

        Self {
            tx: Some(tx),
            handle: Some(handle),
            path,
        }
    }

    /// Returns a clone of the sender for worker threads.
    pub fn sender(&self) -> Sender<ReportRecord> {
        self.tx.as_ref().expect("Reporter already finished").clone()
    }

    /// Stops the reporter and waits for the writer thread to finish.
    pub fn finish(mut self) {
        drop(self.tx.take());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }

        if let Ok(summary) = self.generate_summary() {
            println!("\n{}", "=".repeat(50));
            println!("Conversion Summary:");
            println!("  Report saved to: {}", self.path.display());
            println!("  Total Space Saved: {:.2} MB", summary.space_saved_mb);
            println!(
                "  Average Quality:   {:.2} (SSIMULACRA2)",
                summary.avg_quality
            );
            println!("{}\n", "=".repeat(50));
        }
    }

    /// Consumes report records from the receiver and writes them to a CSV file.
    fn run_writer(rx: Receiver<ReportRecord>, path: PathBuf) {
        let file = match fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to create report file: {}", e);
                return;
            }
        };
        let mut wtr = csv::Writer::from_writer(file);

        while let Ok(record) = rx.recv() {
            if let Err(e) = wtr.serialize(record) {
                eprintln!("Failed to write report record: {}", e);
            }
        }
        let _ = wtr.flush();
    }

    /// Reads the generated CSV report and calculates cumulative statistics.
    fn generate_summary(&self) -> anyhow::Result<ReportSummary> {
        let file = fs::File::open(&self.path)?;
        let mut rdr = csv::Reader::from_reader(file);

        let mut total_orig = 0u64;
        let mut total_final = 0u64;
        let mut quality_sum = 0.0f64;
        let mut count = 0usize;

        for result in rdr.deserialize() {
            let record: ReportRecord = result?;
            total_orig += record.original_size;
            total_final += record.final_size;
            quality_sum += record.quality_value;
            count += 1;
        }

        if count == 0 {
            return Ok(ReportSummary::default());
        }

        let space_saved = total_orig.saturating_sub(total_final);

        Ok(ReportSummary {
            space_saved_mb: space_saved as f64 / 1024.0 / 1024.0,
            avg_quality: quality_sum / count as f64,
        })
    }
}

/// Holds aggregated statistics for the conversion process.
#[derive(Default)]
struct ReportSummary {
    space_saved_mb: f64,
    avg_quality: f64,
}

/// Generates a post-conversion quality report.
pub fn generate_conversion_report(
    source: &Path,
    destination: &Path,
    format: &str,
) -> anyhow::Result<()> {
    println!("\nGenerating quality report...");

    let report_path = if destination.is_dir() {
        destination.join("report.csv")
    } else {
        destination
            .parent()
            .unwrap_or(Path::new("."))
            .join("report.csv")
    };

    let reporter = Reporter::new(report_path);
    let sender = reporter.sender();
    let pairs = collect_comparison_pairs(source, destination, format)?;

    if pairs.is_empty() {
        println!("No matching file pairs found for reporting.");
        return Ok(());
    }

    println!("Comparing {} pairs...", pairs.len());

    let mp = MultiProgress::new();
    let pb = mp.add(ProgressBar::new(pairs.len() as u64));
    pb.set_style(ui::main_bar_style());
    pb.set_message("Quality Analysis");

    pairs
        .par_iter()
        .for_each(|(src_path, dest_path, src_data_loader)| {
            let result = (|| -> anyhow::Result<()> {
                let img_original = src_data_loader()?;
                let original_size = img_original.1;

                let img_distorted = crate::image::load_image(dest_path)?;
                let final_size = fs::metadata(dest_path)?.len();

                let score =
                    compute_quality_from_image(&img_original.0, &img_distorted).unwrap_or(0.0);

                sender.send(ReportRecord::new(
                    src_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    original_size,
                    final_size,
                    score,
                    get_rating(score),
                ))?;
                Ok(())
            })();

            if let Err(e) = result {
                eprintln!(
                    "Error analyzing pair {:?} -> {:?}: {}",
                    src_path,
                    dest_path,
                    e
                );
            }
            pb.inc(1);
        });

    pb.finish_with_message("Analysis Complete");
    drop(sender);
    reporter.finish();

    Ok(())
}

/// Type alias for a closure that loads image data and returns its size.
type DataLoader = Box<dyn Fn() -> anyhow::Result<(image::DynamicImage, u64)> + Sync + Send>;

/// Identifies pairs of original and converted files for quality comparison.
fn collect_comparison_pairs(
    source: &Path,
    destination: &Path,
    format: &str,
) -> anyhow::Result<Vec<(PathBuf, PathBuf, DataLoader)>> {
    let mut pairs = Vec::new();

    if source.is_file() {
        if source
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("cbz"))
        {
            collect_pairs_from_archive(source, destination, format, &mut pairs)?;
        } else {
            collect_pairs_from_file(source, destination, format, &mut pairs)?;
        }
    } else if source.is_dir() {
        collect_pairs_from_directory(source, destination, format, &mut pairs)?;
    }

    Ok(pairs)
}

/// Scans an archive and matches its internal images with converted files in the destination.
fn collect_pairs_from_archive(
    source: &Path,
    destination: &Path,
    format: &str,
    pairs: &mut Vec<(PathBuf, PathBuf, DataLoader)>,
) -> anyhow::Result<()> {
    if !destination.is_dir() {
        println!("Warning: Source is Archive but Destination is File. Cannot compare.");
        return Ok(())
    }

    let file = fs::File::open(source)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        if file.is_file() {
            let name = file.name().to_string();
            let path = Path::new(&name);
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                let dest_file = destination.join(path).with_extension(format);
                if dest_file.exists() {
                    let src_path_clone = source.to_path_buf();
                    let name_clone = name.clone();
                    let loader: DataLoader = Box::new(move || {
                        let f = fs::File::open(&src_path_clone)?;
                        let mut a = zip::ZipArchive::new(f)?;
                        let mut entry = a.by_name(&name_clone)?;
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut entry, &mut buf)?;
                        let len = buf.len() as u64;
                        let img = image::load_from_memory(&buf)?;
                        Ok((img, len))
                    });
                    pairs.push((PathBuf::from(name), dest_file, loader));
                }
            }
        }
    }
    Ok(())
}

/// Matches a single source file with its converted counterpart.
fn collect_pairs_from_file(
    source: &Path,
    destination: &Path,
    format: &str,
    pairs: &mut Vec<(PathBuf, PathBuf, DataLoader)>,
) -> anyhow::Result<()> {
    if destination.is_file() {
        let src_path_clone = source.to_path_buf();
        let loader: DataLoader = Box::new(move || {
            let buf = fs::read(&src_path_clone)?;
            let len = buf.len() as u64;
            let img = image::load_from_memory(&buf)?;
            Ok((img, len))
        });
        pairs.push((source.to_path_buf(), destination.to_path_buf(), loader));
    } else if destination.is_dir() {
        let file_name = source.file_name().unwrap();
        let dest_file = destination.join(file_name).with_extension(format);
        if dest_file.exists() {
            let src_path_clone = source.to_path_buf();
            let loader: DataLoader = Box::new(move || {
                let buf = fs::read(&src_path_clone)?;
                let len = buf.len() as u64;
                let img = image::load_from_memory(&buf)?;
                Ok((img, len))
            });
            pairs.push((source.to_path_buf(), dest_file, loader));
        }
    }
    Ok(())
}

/// Recursively scans a directory to find pairs of original and converted images.
fn collect_pairs_from_directory(
    source: &Path,
    destination: &Path,
    format: &str,
    pairs: &mut Vec<(PathBuf, PathBuf, DataLoader)>,
) -> anyhow::Result<()> {
    if !destination.is_dir() {
        println!("Warning: Source is Directory but Destination is File. Cannot compare.");
        return Ok(())
    }

    for entry in WalkDir::new(source) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                let rel_path = path.strip_prefix(source)?;
                let dest_file = destination.join(rel_path).with_extension(format);

                if dest_file.exists() {
                    let src_path_clone = path.to_path_buf();
                    let loader: DataLoader = Box::new(move || {
                        let buf = fs::read(&src_path_clone)?;
                        let len = buf.len() as u64;
                        let img = image::load_from_memory(&buf)?;
                        Ok((img, len))
                    });
                    pairs.push((path.to_path_buf(), dest_file, loader));
                }
            }
        }
    }
    Ok(())
}