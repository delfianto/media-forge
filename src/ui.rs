use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub const SPINNER_TICK: Duration = Duration::from_millis(100);

/// Returns the standard style for indeterminate spinners (e.g., scanning).
pub fn scanner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg} {pos} items found")
        .expect("Invalid template")
}

/// Returns a generic spinner style for worker threads.
pub fn generic_spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .expect("Invalid template")
}

/// Returns the standard style for analysis spinners.
pub fn analyzing_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.blue} Analyzing: {msg}")
        .expect("Invalid template")
}

/// Returns the standard style for main task progress bars.
pub fn main_bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )
        .expect("Invalid template")
        .progress_chars("#>-")
}

/// Returns the standard style for sub-task/container progress bars.
pub fn sub_bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {bar:30.magenta/blue} {pos}/{len} {msg}")
        .expect("Invalid template")
        .progress_chars("=>-")
}

/// Returns the standard style for single file progress (e.g. video encoding).
pub fn file_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {msg}\n  {bar:40.magenta/blue} {percent}% {eta}")
        .expect("Invalid template")
        .progress_chars("=>-")
}

/// Creates a standard scanner progress bar.
pub fn create_scanner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(scanner_style());
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(SPINNER_TICK);
    pb
}
