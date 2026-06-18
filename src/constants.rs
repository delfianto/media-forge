//! Application-wide constants used across the media-forge project.

/// Default CPU utilization ratio (75%)
pub const DEFAULT_CPU_RATIO: f64 = 0.75;

/// Maximum CPU utilization ratio (150%)
pub const MAX_CPU_RATIO: f64 = 1.5;

/// Channel buffer multiplier for work distribution
pub const CHANNEL_BUFFER_MULTIPLIER: usize = 2;

/// Maximum spinner count for analysis phase
pub const MAX_ANALYSIS_SPINNERS: usize = 8;

/// Report channel buffer size
pub const REPORT_CHANNEL_CAPACITY: usize = 1000;

/// VMAF error log path
pub const VMAF_ERROR_LOG: &str = "media-forge-vmaf-error.log";

/// Progress bar tick interval in milliseconds
pub const SPINNER_TICK_MS: u64 = 100;

/// Default quality for image conversion
pub const DEFAULT_IMAGE_QUALITY: u8 = 80;

/// Default speed for AVIF encoding
pub const DEFAULT_AVIF_SPEED: u8 = 4;

/// Default recursion depth for file scanning
pub const DEFAULT_RECURSION_DEPTH: usize = 2;

/// Default CQ level for video encoding
pub const DEFAULT_VIDEO_CQ: u8 = 28;

/// Default VMAF analysis duration in seconds
pub const DEFAULT_VMAF_DURATION: u64 = 60;
