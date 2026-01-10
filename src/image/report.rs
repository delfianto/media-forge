use crossbeam_channel::{Receiver, Sender, bounded};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::thread::JoinHandle;

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
        drop(self.tx.take()); // Close the channel
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

    fn generate_summary(&self) -> anyhow::Result<ReportSummary> {
        // Read the CSV back to generate a summary
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

#[derive(Default)]
struct ReportSummary {
    space_saved_mb: f64,
    avg_quality: f64,
}
