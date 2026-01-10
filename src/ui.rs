use crate::constants::SPINNER_TICK_MS;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Tick rate for indeterminate spinners.
pub const SPINNER_TICK: Duration = Duration::from_millis(SPINNER_TICK_MS);

/// Progress bar characters for a shaded look (filled, current, empty).
pub const PROGRESS_CHARS: &str = "█▓░";

/// Returns the standard style for indeterminate spinners used during file scanning.
pub fn scanner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .expect("Invalid template")
}

/// Returns a generic spinner style for worker threads to indicate activity.
pub fn generic_spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .expect("Invalid template")
}

/// Returns the standard style for analysis spinners used during task planning.
pub fn analyzing_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.blue} Analyzing: {msg}")
        .expect("Invalid template")
}

/// Returns the standard style for main task progress bars.
pub fn main_bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan}] {pos}/{len} ({eta}) {msg}")
        .expect("Invalid template")
        .progress_chars(PROGRESS_CHARS)
}

/// Returns the standard style for sub-task or container-level progress bars.
pub fn sub_bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  [{bar:30.yellow}] {pos}/{len} {msg}")
        .expect("Invalid template")
        .progress_chars(PROGRESS_CHARS)
}

/// Returns the standard style for single file progress, typically used for long-running video encoding.
pub fn file_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {msg}\n  [{bar:40.yellow}] {percent}% {eta}")
        .expect("Invalid template")
        .progress_chars(PROGRESS_CHARS)
}

/// Creates and initializes a standard scanner progress bar with a message.
pub fn create_scanner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(scanner_style());
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(SPINNER_TICK);
    pb
}
