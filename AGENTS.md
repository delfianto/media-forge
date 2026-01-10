# AGENTS.md - LLM-Assisted Refactoring Guide for Media-Forge

> **Purpose**: This document provides comprehensive guidance for LLM agents performing refactoring, bug fixes, feature additions, and code improvements on the media-forge codebase.
>
> **Last Updated**: 2026-01-10 (Post-Refactoring Analysis)

---

## Table of Contents

1. [Project Overview](#project-overview)
2. [Architecture](#architecture)
3. [Codebase Structure](#codebase-structure)
4. [Module Deep-Dive](#module-deep-dive)
5. [Key Patterns and Conventions](#key-patterns-and-conventions)
6. [Error Handling](#error-handling)
7. [Concurrency Model](#concurrency-model)
8. [Dependencies](#dependencies)
9. [Recent Refactoring Summary](#recent-refactoring-summary)
10. [Remaining Refactoring Targets](#remaining-refactoring-targets)
11. [Testing Strategy](#testing-strategy)
12. [Common Pitfalls](#common-pitfalls)
13. [Code Style Guidelines](#code-style-guidelines)

---

## Project Overview

**Media-Forge** is a high-performance Rust CLI tool for batch media conversion on Linux systems.

### Core Capabilities

| Feature | Description | External Dependencies |
|---------|-------------|----------------------|
| Image Conversion | Convert images to AVIF/WebP with quality control | `ravif`, `image` crates |
| Video Encoding | Hardware-accelerated AV1 encoding via NVIDIA NVENC | FFmpeg with CUDA support |
| CBZ Archive Creation | Create comic book archives from image folders | `zip` crate |
| Quality Analysis | SSIMULACRA2 for images, VMAF for videos | `ssimulacra2` crate, FFmpeg libvmaf |

### Design Philosophy

1. **Performance-First**: MiMalloc allocator, Rayon parallelism, 75% CPU utilization default
2. **Safety**: Graceful shutdown via Ctrl+C, process tracking, archive verification
3. **User Experience**: Multi-level progress bars, dry-run modes, natural sorting
4. **Modularity**: Clean separation between image/video/archive processing

---

## Architecture

### High-Level Data Flow

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                              CLI Entry (main.rs)                              │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌─────────┐ │
│  │   image    │  │  archive   │  │   video    │  │ simulacra  │  │  vmaf   │ │
│  │  command   │  │  command   │  │  command   │  │  command   │  │ command │ │
│  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └────┬────┘ │
└────────┼───────────────┼───────────────┼───────────────┼──────────────┼──────┘
         │               │               │               │              │
         ▼               ▼               ▼               ▼              ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                           Walker (walker.rs)                                  │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────────┐   │
│  │ Filesystem Scan │  │ ZIP/CBZ Extract │  │ MediaSource Abstraction    │   │
│  │ (WalkDir crate) │  │ (zip crate)     │  │ (Filesystem | Archive)     │   │
│  └─────────────────┘  └─────────────────┘  └─────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                        Processing Layer                                       │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────────┐    │
│  │ image/convert.rs │  │ image/archive.rs │  │ video/encode.rs          │    │
│  │  - Task creation │  │  - CBZ creation  │  │  - FfmpegCommandBuilder  │    │
│  │  - AVIF/WebP enc │  │  - Verification  │  │  - Progress parsing      │    │
│  │  - Parallel exec │  │  - Cleanup       │  │  - Process management    │    │
│  └──────────────────┘  └──────────────────┘  └──────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                        Quality Analysis Layer                                 │
│  ┌────────────────────────────┐  ┌────────────────────────────────────────┐  │
│  │ image/quality.rs           │  │ video/quality.rs                       │  │
│  │  - SSIMULACRA2 scoring     │  │  - VMAF via FFmpeg libvmaf             │  │
│  │  - Pair matching           │  │  - Frame subsampling                   │  │
│  └────────────────────────────┘  └────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                        Support Layers                                         │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────────┐   │
│  │ ui.rs           │  │ constants.rs    │  │ video/builder.rs           │   │
│  │ Progress Styles │  │ Magic Numbers   │  │ FFmpeg Command Builder     │   │
│  └─────────────────┘  └─────────────────┘  └─────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Process Management Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Shutdown Coordination                     │
│                                                             │
│   ┌─────────────────┐    ┌─────────────────────────────┐    │
│   │ SHUTDOWN flag   │◄───│ Ctrl+C Handler (ctrlc)      │    │
│   │ (AtomicBool)    │    │ Sets flag + kills processes │    │
│   └────────┬────────┘    └─────────────────────────────┘    │
│            │                                                │
│            ▼                                                │
│   ┌─────────────────────────────────────────────────────┐   │
│   │              ProcessManager                          │   │
│   │  ┌───────────────────────────────────────────────┐  │   │
│   │  │ ACTIVE_PROCESSES: Lazy<Mutex<HashSet<u32>>>   │  │   │
│   │  │  - register(pid)   → adds PID to set          │  │   │
│   │  │  - unregister(pid) → removes PID from set     │  │   │
│   │  │  - kill_all()      → SIGKILL all tracked PIDs │  │   │
│   │  └───────────────────────────────────────────────┘  │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                             │
│   Worker threads check SHUTDOWN flag before each task       │
│   FFmpeg stderr reader checks SHUTDOWN in read loop         │
└─────────────────────────────────────────────────────────────┘
```

---

## Codebase Structure

```
media-forge/
├── Cargo.toml                 # Rust 2024 edition, dependencies
├── AGENTS.md                  # This file - LLM guidance
├── src/
│   ├── main.rs                # CLI entry point, command routing (92 lines)
│   ├── lib.rs                 # Core utilities, ProcessManager (234 lines)
│   ├── constants.rs           # ✅ NEW: Extracted magic numbers (38 lines)
│   ├── walker.rs              # File/archive scanning abstraction (286 lines)
│   ├── ui.rs                  # Progress bar styles and factories (61 lines)
│   ├── image/
│   │   ├── mod.rs             # ImageError, Args, ConversionSummary (247 lines)
│   │   ├── convert.rs         # Image conversion pipeline (591 lines)
│   │   ├── archive.rs         # CBZ archive creation (374 lines)
│   │   ├── quality.rs         # SSIMULACRA2 scoring (~123 lines)
│   │   └── report.rs          # Quality report generation (434 lines)
│   └── video/
│       ├── mod.rs             # VideoError, Args, metadata (269 lines)
│       ├── builder.rs         # ✅ NEW: FFmpeg command builder (203 lines)
│       ├── encode.rs          # FFmpeg video encoding (292 lines)
│       └── quality.rs         # VMAF quality analysis (278 lines)
└── tests/
    ├── image_convert.rs       # ✅ NEW: Image conversion integration tests
    └── archive_create.rs      # ✅ NEW: Archive creation integration tests
```

### Lines of Code by Module (Updated)

| Module | Lines | Purpose | Test Coverage |
|--------|-------|---------|---------------|
| `src/image/convert.rs` | 591 | Image conversion pipeline | ✅ Integration tests |
| `src/image/report.rs` | 434 | Quality report generation | ✅ 1 unit test |
| `src/image/archive.rs` | 374 | CBZ archive creation | ✅ Integration tests |
| `src/video/encode.rs` | 292 | FFmpeg video encoding | ❌ None (requires FFmpeg) |
| `src/walker.rs` | 286 | File/archive scanning | ✅ 4 unit tests |
| `src/video/quality.rs` | 278 | VMAF analysis | ❌ None (requires FFmpeg) |
| `src/video/mod.rs` | 269 | Video error types, metadata | ✅ 2 unit tests |
| `src/image/mod.rs` | 247 | Image error types, args | ❌ None |
| `src/lib.rs` | 234 | Core utilities | ✅ 8 unit tests |
| `src/video/builder.rs` | 203 | FFmpeg command builder | ✅ 3 unit tests |
| `src/main.rs` | 92 | CLI entry | ❌ None |
| `src/ui.rs` | 61 | Progress styles | ❌ None |
| `src/constants.rs` | 38 | Application constants | ❌ None |

**Total**: ~3,399 lines with **20+ tests** (significant improvement from 5 tests)

---

## Module Deep-Dive

### `src/constants.rs` - Application Constants ✅ NEW

All magic numbers have been extracted to a centralized constants module:

```rust
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

/// VMAF error log path (relative, not /tmp)
pub const VMAF_ERROR_LOG: &str = "media-forge-vmaf-error.log";

/// Progress bar tick interval in milliseconds
pub const SPINNER_TICK_MS: u64 = 100;

/// Default quality for image conversion
pub const DEFAULT_IMAGE_QUALITY: u8 = 72;

/// Default speed for AVIF encoding
pub const DEFAULT_AVIF_SPEED: u8 = 4;

/// Default recursion depth for file scanning
pub const DEFAULT_RECURSION_DEPTH: usize = 2;

/// Default CQ level for video encoding
pub const DEFAULT_VIDEO_CQ: u8 = 28;

/// Default VMAF analysis duration in seconds
pub const DEFAULT_VMAF_DURATION: u64 = 60;
```

**Usage Pattern**:
```rust
use crate::constants::{DEFAULT_CPU_RATIO, MAX_CPU_RATIO, CHANNEL_BUFFER_MULTIPLIER};
```

---

### `src/video/builder.rs` - FFmpeg Command Builder ✅ NEW

A fluent API for constructing FFmpeg commands:

```rust
pub enum HwAccel {
    Cuda,
}

pub enum VideoCodec {
    Av1Nvenc,
}

pub struct FfmpegCommandBuilder {
    input: PathBuf,
    output: PathBuf,
    hwaccel: Option<HwAccel>,
    video_codec: Option<VideoCodec>,
    preset: Option<String>,
    cq: Option<u8>,
    extra_args: Vec<String>,
    start_time: Option<u64>,
    duration: Option<f64>,
}

impl FfmpegCommandBuilder {
    pub fn new(input: &Path, output: &Path) -> Self { ... }
    pub fn hwaccel(mut self, hwaccel: HwAccel) -> Self { ... }
    pub fn video_codec(mut self, codec: VideoCodec) -> Self { ... }
    pub fn preset(mut self, preset: &str) -> Self { ... }
    pub fn cq(mut self, cq: u8) -> Self { ... }
    pub fn start_time(mut self, seconds: u64) -> Self { ... }
    pub fn duration(mut self, seconds: f64) -> Self { ... }
    pub fn arg(mut self, arg: &str) -> Self { ... }
    pub fn args<I, S>(mut self, args: I) -> Self { ... }
    pub fn build(self) -> Command { ... }
}
```

**Usage in encode.rs**:
```rust
let mut builder = FfmpegCommandBuilder::new(&task.src, &task.dest)
    .hwaccel(HwAccel::Cuda)
    .video_codec(VideoCodec::Av1Nvenc)
    .preset(&args.preset)
    .cq(args.cq)
    .args(["-tune", "hq", "-c:a", "copy"]);

if args.ext == "mp4" {
    builder = builder.args(["-c:s", "mov_text"]);
}

let mut cmd = builder.build();
```

**Tests** (3 unit tests):
- `test_builder_basic` - Core functionality
- `test_builder_time_and_duration` - Time-based options
- `test_builder_extra_args` - Additional argument handling

---

### `src/main.rs` - CLI Entry Point (Improved)

**Key Improvement**: Now uses `try_set_handler()` with proper error propagation:

```rust
fn main() -> Result<()> {
    ctrlc::try_set_handler(move || {
        eprintln!("\n\x1b[31m[Interrupt] Shutting down...\x1b[0m");
        ProcessManager::kill_all();
        std::process::exit(130);
    })?;  // ✅ Now uses ? instead of .expect()

    let cli = Cli::parse();
    match cli.command {
        Commands::Image(args) => image::run(args),
        Commands::Archive(args) => image::run_archive(args),
        // ...
    }
}
```

---

### `src/image/mod.rs` - ConversionSummary ✅ NEW

Error aggregation is now implemented:

```rust
/// Holds aggregated results of a batch conversion process.
pub struct ConversionSummary {
    /// Total number of assets processed.
    pub total: usize,
    /// Number of successfully converted files.
    pub succeeded: usize,
    /// Number of files skipped (e.g., already exists).
    pub skipped: usize,
    /// List of paths and their associated error messages for failed conversions.
    pub failed: Vec<(PathBuf, String)>,
}

impl ConversionSummary {
    /// Prints a formatted summary of the conversion results to the console.
    pub fn print_summary(&self) {
        println!("\n{}", "=".repeat(50));
        println!("Conversion Summary:");
        println!("  Total Assets: {}", self.total);
        println!("  ✓ Succeeded:  {}", self.succeeded);
        println!("  → Skipped:    {}", self.skipped);
        if !self.failed.is_empty() {
            println!("  ✗ Failed:     {}", self.failed.len());
            for (path, error) in &self.failed {
                println!("    - {:?}: {}", path, error);
            }
        }
        println!("{}\n", "=".repeat(50));
    }

    /// Returns a non-zero exit code if any conversions failed.
    pub fn exit_code(&self) -> i32 {
        if self.failed.is_empty() { 0 } else { 1 }
    }
}
```

---

### `src/walker.rs` - File Scanning (Enhanced)

Now includes comprehensive unit tests:

```rust
#[cfg(test)]
mod tests {
    #[test] fn test_walker_scan_flat() { ... }      // Basic file scanning
    #[test] fn test_walker_recursive() { ... }      // Recursive directory traversal
    #[test] fn test_walker_archive_inspection() { ... }  // ZIP/CBZ content scanning
    #[test] fn test_scan_grouped() { ... }          // Grouping by parent directory
}
```

---

### `src/image/convert.rs` - Image Conversion Pipeline (Improved)

**Key Improvements**:

1. **Safe mutex locking** (line 131-133):
```rust
let pool = spinner_pool
    .lock()
    .map_err(|_| ImageError::Io(std::io::Error::other("Lock poisoned")))?;
```

2. **Safe spinner pool access** (line 182, 216-220, 425, 437-443):
```rust
let pb_opt = spinner_pool.lock().ok().and_then(|mut pool| pool.pop());

// Return spinner to pool safely
if let Some(pb) = pb_opt
    && let Ok(mut pool) = spinner_pool.lock()
{
    pool.push(pb);
}
```

3. **Error aggregation** (line 365-379):
```rust
let mut summary = ConversionSummary {
    total: total_files,
    succeeded: 0,
    skipped: 0,
    failed: Vec::new(),
};

while let Ok((path, res)) = results_rx.try_recv() {
    match res {
        Ok(()) => summary.succeeded += 1,
        Err(e) => summary.failed.push((path, e.to_string())),
    }
}
```

**Remaining Unsafe Code** (line 579-581):
```rust
// AVIF encoding with unsafe pointer cast - STILL NEEDS REVIEW
let rgba_pixels = unsafe {
    std::slice::from_raw_parts(pixels.as_ptr() as *const ravif::RGBA8, pixels.len() / 4)
};
```

---

## Recent Refactoring Summary

### ✅ Completed Improvements

| Priority | Issue | Status | Details |
|----------|-------|--------|---------|
| P1 | Unsafe unwrap in main.rs | ✅ FIXED | Now uses `try_set_handler()?` |
| P1 | Mutex lock unwraps | ✅ FIXED | Now uses `.lock().ok()` or `.map_err()` |
| P2 | Add integration tests | ✅ DONE | 2 test files, multiple tests |
| P3 | Error aggregation | ✅ DONE | `ConversionSummary` implemented |
| P4 | Extract magic numbers | ✅ DONE | `src/constants.rs` created |
| P5 | FFmpeg command builder | ✅ DONE | `src/video/builder.rs` created |

### Test Coverage Improvement

| Before | After |
|--------|-------|
| 5 unit tests | 20+ tests |
| 0 integration tests | 2 integration test files |
| ~0.18% coverage | Significantly improved |

---

## Remaining Refactoring Targets

### Summary Table

| ID | Location | Pattern | Risk | Effort | Status |
|----|----------|---------|------|--------|--------|
| R1 | `image/convert.rs:579-581` | `unsafe { std::slice::from_raw_parts(...) }` | MEDIUM | Medium | Pending |
| R2 | `image/report.rs:76` | `.expect("Reporter already finished")` | MEDIUM | Low | Pending |
| R3 | `image/report.rs:324` | `file_name().unwrap()` | LOW | Trivial | Pending |
| R4 | `video/encode.rs:116` | `file_name().unwrap()` | LOW | Trivial | Pending |

---

## Detailed Refactoring Plan

### R1: Remove Unsafe Pointer Cast in AVIF Encoding

**File**: `src/image/convert.rs`
**Lines**: 579-581
**Risk**: MEDIUM
**Effort**: Medium (requires dependency addition)

#### Current Code
```rust
let pixels = img_rgba.as_raw();
let rgba_pixels = unsafe {
    std::slice::from_raw_parts(pixels.as_ptr() as *const ravif::RGBA8, pixels.len() / 4)
};
```

#### Problem Analysis
- Uses `unsafe` to reinterpret `&[u8]` as `&[ravif::RGBA8]`
- Assumes memory layout compatibility between `u8` array and `RGBA8` struct
- Risk: If `ravif::RGBA8` has different alignment or padding, this causes UB
- The cast `pixels.len() / 4` assumes exactly 4 bytes per pixel (RGBA)

#### Solution Options

**Option A: Use `bytemuck` crate (Recommended)**

1. Add dependency to `Cargo.toml`:
```toml
[dependencies]
bytemuck = { version = "1.14", features = ["derive"] }
```

2. Verify `ravif::RGBA8` is `bytemuck`-compatible (check if it's `#[repr(C)]`):
```rust
// If ravif::RGBA8 is already Pod-compatible:
use bytemuck::cast_slice;

let pixels = img_rgba.as_raw();
let rgba_pixels: &[ravif::RGBA8] = cast_slice(pixels);
```

3. If `ravif::RGBA8` is not `Pod`, create a wrapper:
```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Rgba8 {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

// Then cast and convert
let rgba_pixels: &[Rgba8] = bytemuck::cast_slice(pixels);
// Map to ravif::RGBA8 if needed
```

**Option B: Use `rgb` crate**

1. The `image` crate already uses `rgb` internally. Check if we can use:
```rust
use rgb::FromSlice;

let pixels = img_rgba.as_raw();
let rgba_pixels = pixels.as_rgba();  // Returns &[RGBA<u8>]
```

2. Verify compatibility with `ravif::Img::new()` expectations.

**Option C: Safe Manual Conversion (No new deps, slower)**

```rust
let pixels = img_rgba.as_raw();
let rgba_pixels: Vec<ravif::RGBA8> = pixels
    .chunks_exact(4)
    .map(|chunk| ravif::RGBA8 {
        r: chunk[0],
        g: chunk[1],
        b: chunk[2],
        a: chunk[3],
    })
    .collect();

let img_ravif = ravif::Img::new(&rgba_pixels, width as usize, height as usize);
```

#### Recommended Implementation

```rust
/// Encodes an image to AVIF format with hardware-independent encoding.
fn encode_avif(img: &DynamicImage, dest: &Path, quality: u8, speed: u8) -> Result<()> {
    let (width, height) = img.dimensions();
    let img_rgba = img.to_rgba8();

    let enc = ravif::Encoder::new()
        .with_quality(quality as f32)
        .with_speed(speed);

    let pixels = img_rgba.as_raw();

    // SAFETY FIX: Use bytemuck for safe type punning
    // Verify at compile time that the types are compatible
    let rgba_pixels: &[ravif::RGBA8] = bytemuck::cast_slice(pixels);

    let img_ravif = ravif::Img::new(rgba_pixels, width as usize, height as usize);
    let out = enc
        .encode_rgba(img_ravif)
        .map_err(|e| ImageError::AvifEncoding(e.to_string()))?;

    fs::write(dest, out.avif_file)?;
    Ok(())
}
```

#### Testing Requirements
1. Verify existing image conversion tests still pass
2. Test with various image dimensions (odd sizes, 1x1, large images)
3. Test with images that have alpha channel variations
4. Benchmark to ensure no performance regression

#### Verification Checklist
- [ ] Add `bytemuck` to `Cargo.toml`
- [ ] Verify `ravif::RGBA8` layout compatibility
- [ ] Remove `unsafe` block
- [ ] Run `cargo test`
- [ ] Run `cargo clippy` - no new warnings
- [ ] Test manual AVIF conversion with sample images

---

### R2: Fix Reporter `.expect()` Call

**File**: `src/image/report.rs`
**Lines**: 76
**Risk**: MEDIUM
**Effort**: Low

#### Current Code
```rust
/// Returns a clone of the sender for worker threads.
pub fn sender(&self) -> Sender<ReportRecord> {
    self.tx.as_ref().expect("Reporter already finished").clone()
}
```

#### Problem Analysis
- Panics if called after `finish()` has been called
- The `finish()` method takes `self` by value, so this *should* be unreachable
- However, the API allows for misuse if someone clones the Reporter or uses unsafe
- Better to return `Option` or `Result` for defensive programming

#### Solution Options

**Option A: Return `Option<Sender<ReportRecord>>` (Recommended)**

```rust
/// Returns a clone of the sender for worker threads.
///
/// Returns `None` if the reporter has already been finished.
pub fn sender(&self) -> Option<Sender<ReportRecord>> {
    self.tx.as_ref().cloned()
}
```

Update call sites:
```rust
// In generate_conversion_report():
let sender = reporter.sender().ok_or_else(|| {
    anyhow::anyhow!("Reporter was unexpectedly finished")
})?;
```

**Option B: Return `Result<Sender<ReportRecord>, ReportError>`**

1. Add error variant to `ImageError`:
```rust
#[derive(Error, Debug)]
pub enum ImageError {
    // ... existing variants ...

    #[error("Reporter has already been finished")]
    ReporterFinished,
}
```

2. Update method:
```rust
pub fn sender(&self) -> Result<Sender<ReportRecord>, ImageError> {
    self.tx
        .as_ref()
        .cloned()
        .ok_or(ImageError::ReporterFinished)
}
```

**Option C: Debug Assert (Minimal change)**

```rust
pub fn sender(&self) -> Sender<ReportRecord> {
    debug_assert!(self.tx.is_some(), "Reporter already finished");
    self.tx
        .as_ref()
        .expect("Reporter already finished - this is a bug")
        .clone()
}
```

#### Recommended Implementation (Option A)

```rust
/// Returns a clone of the sender for worker threads.
///
/// Returns `None` if the reporter has already been finished.
/// This should not happen in normal usage since `finish()` consumes `self`.
pub fn sender(&self) -> Option<Sender<ReportRecord>> {
    self.tx.as_ref().cloned()
}
```

```rust
// In generate_conversion_report() at line 174:
let sender = match reporter.sender() {
    Some(s) => s,
    None => {
        eprintln!("Warning: Reporter was unexpectedly finished");
        return Ok(());
    }
};
```

#### Testing Requirements
1. Existing `test_report_summary` should still pass
2. Add test verifying `sender()` returns `Some` for active reporter
3. No new test needed for `None` case (requires `finish()` which consumes self)

#### Verification Checklist
- [ ] Change return type to `Option<Sender<ReportRecord>>`
- [ ] Update `generate_conversion_report()` call site
- [ ] Run `cargo test`
- [ ] Run `cargo clippy`

---

### R3: Fix `file_name().unwrap()` in Report

**File**: `src/image/report.rs`
**Lines**: 324
**Risk**: LOW
**Effort**: Trivial

#### Current Code
```rust
} else if destination.is_dir() {
    let file_name = source.file_name().unwrap();
    let dest_file = destination.join(file_name).with_extension(format);
```

#### Problem Analysis
- `file_name()` returns `None` for paths ending in `..` or root paths like `/`
- Unlikely in practice since this is called on validated source files
- Still better to handle defensively

#### Solution

```rust
} else if destination.is_dir() {
    let file_name = match source.file_name() {
        Some(name) => name,
        None => {
            // Path has no file name (e.g., "/" or ends with "..")
            // Skip this pair silently
            return Ok(());
        }
    };
    let dest_file = destination.join(file_name).with_extension(format);
```

Or more concisely:
```rust
} else if destination.is_dir() {
    let Some(file_name) = source.file_name() else {
        return Ok(()); // Skip paths without file names
    };
    let dest_file = destination.join(file_name).with_extension(format);
```

#### Testing Requirements
- Add unit test with edge case path (if feasible)
- Existing tests should pass

#### Verification Checklist
- [ ] Replace `.unwrap()` with `let Some(...) else { return Ok(()); }`
- [ ] Run `cargo test`
- [ ] Run `cargo clippy`

---

### R4: Fix `file_name().unwrap()` in Video Encode

**File**: `src/video/encode.rs`
**Lines**: 116
**Risk**: LOW
**Effort**: Trivial

#### Current Code
```rust
let dest_file = if source_path.is_file() {
    if dest_path.extension().is_some() {
        dest_path.to_path_buf()
    } else {
        dest_path
            .join(file.file_name().unwrap())
            .with_extension(&args.ext)
    }
} else {
```

#### Problem Analysis
- `file.file_name()` is called on a path from `MediaSource::Filesystem`
- The file was already validated to exist and be a file
- Still could fail on unusual paths, better to handle defensively

#### Solution

```rust
let dest_file = if source_path.is_file() {
    if dest_path.extension().is_some() {
        dest_path.to_path_buf()
    } else {
        let file_name = file.file_name().unwrap_or_default();
        dest_path.join(file_name).with_extension(&args.ext)
    }
} else {
```

Or with early continue for the parallel iterator:
```rust
let dest_file = if source_path.is_file() {
    if dest_path.extension().is_some() {
        dest_path.to_path_buf()
    } else {
        let Some(file_name) = file.file_name() else {
            pb_scan.inc(1);
            return; // Skip files without valid names
        };
        dest_path.join(file_name).with_extension(&args.ext)
    }
} else {
```

#### Recommended Implementation
Use `unwrap_or_default()` since we want to continue processing:

```rust
dest_path
    .join(file.file_name().unwrap_or_default())
    .with_extension(&args.ext)
```

#### Testing Requirements
- Existing tests should pass
- No new tests needed (edge case is extremely rare)

#### Verification Checklist
- [ ] Replace `.unwrap()` with `.unwrap_or_default()`
- [ ] Run `cargo test`
- [ ] Run `cargo clippy`

---

## Implementation Order

Recommended order for implementing these fixes:

| Order | ID | Reason |
|-------|-----|--------|
| 1 | R3 | Trivial fix, immediate improvement |
| 2 | R4 | Trivial fix, immediate improvement |
| 3 | R2 | Low effort, improves API safety |
| 4 | R1 | Medium effort, requires dependency evaluation |

### Batch Commit Strategy

**Commit 1**: Fix trivial unwraps (R3 + R4)
```
fix: replace unwrap() with safe alternatives in report.rs and encode.rs

- report.rs:324: Use let-else pattern for file_name()
- encode.rs:116: Use unwrap_or_default() for file_name()
```

**Commit 2**: Fix Reporter API (R2)
```
refactor: return Option from Reporter::sender()

- Change sender() return type from Sender to Option<Sender>
- Update call site in generate_conversion_report()
- Provides defensive API without expect() panic
```

**Commit 3**: Remove unsafe from AVIF encoding (R1)
```
refactor: remove unsafe block in AVIF encoding

- Add bytemuck dependency for safe type punning
- Replace unsafe slice::from_raw_parts with bytemuck::cast_slice
- Verify RGBA8 layout compatibility at compile time
```

---

## Additional Refactoring Opportunities

### Priority 2: Add More Tests

**Still Needed**:
1. **Video encoding tests** (requires FFmpeg mock or skip in CI)
2. **VMAF analysis tests** (requires FFmpeg with libvmaf)
3. **Error path tests** (invalid inputs, permission errors)
4. **Edge case tests** (empty directories, corrupt archives)

### Priority 3: Documentation

1. Add `#[doc]` comments to public APIs in `constants.rs`
2. Add usage examples in module-level documentation
3. Consider generating API docs with `cargo doc`

### Priority 4: Extend FFmpeg Builder

The current builder supports CUDA/AV1 only. Consider adding:
```rust
pub enum HwAccel {
    Cuda,
    Vaapi,      // For AMD/Intel
    Qsv,        // Intel QuickSync
    None,       // Software encoding
}

pub enum VideoCodec {
    Av1Nvenc,
    HevcNvenc,
    H264Nvenc,
    Libx265,    // Software
    Copy,
}
```

---

## Testing Strategy

### Current Test Structure

```
tests/
├── image_convert.rs     # ✅ Integration tests for image conversion
│   ├── test_image_conversion_integration()
│   └── test_image_conversion_preserve_mtime()
└── archive_create.rs    # ✅ Integration tests for CBZ creation
    └── test_archive_creation_integration()

src/
├── lib.rs               # 8 unit tests
├── walker.rs            # 4 unit tests
├── video/
│   ├── mod.rs           # 2 unit tests (regex parsing)
│   └── builder.rs       # 3 unit tests
└── image/
    └── report.rs        # 1 unit test
```

### Running Tests

```bash
# Run all tests (requires dav1d library for AVIF)
cargo test

# Run specific test file
cargo test --test image_convert

# Run with verbose output
cargo test -- --nocapture

# Skip tests requiring external dependencies
cargo test --features skip-external-deps
```

### System Dependencies for Tests

| Dependency | Required For | Installation |
|------------|--------------|--------------|
| `dav1d` | AVIF decoding | `apt install libdav1d-dev` |
| `FFmpeg` | Video tests | `apt install ffmpeg` |
| `libvmaf` | VMAF tests | Build FFmpeg with `--enable-libvmaf` |

---

## Key Patterns and Conventions

### 1. Constants Usage Pattern

```rust
// Import from centralized constants
use crate::constants::{
    DEFAULT_CPU_RATIO,
    MAX_CPU_RATIO,
    CHANNEL_BUFFER_MULTIPLIER,
    DEFAULT_IMAGE_QUALITY,
};

// Use in code
let default_threads = (total_cpus as f64 * DEFAULT_CPU_RATIO).ceil() as usize;
let (tx, rx) = bounded::<WorkItem>(num_threads * CHANNEL_BUFFER_MULTIPLIER);
```

### 2. Safe Mutex Access Pattern

```rust
// For fallible operations - use .ok() with and_then
let pb_opt = spinner_pool.lock().ok().and_then(|mut pool| pool.pop());

// For operations that must succeed - use map_err
let pool = spinner_pool
    .lock()
    .map_err(|_| ImageError::Io(std::io::Error::other("Lock poisoned")))?;

// For fire-and-forget operations
if let Ok(mut pool) = spinner_pool.lock() {
    pool.push(pb);
}
```

### 3. FFmpeg Command Builder Pattern

```rust
// Fluent API for building FFmpeg commands
let cmd = FfmpegCommandBuilder::new(input, output)
    .hwaccel(HwAccel::Cuda)
    .video_codec(VideoCodec::Av1Nvenc)
    .preset("p6")
    .cq(28)
    .args(["-tune", "hq"])
    .duration(60.0)
    .build();
```

### 4. Error Aggregation Pattern

```rust
// Collect errors during parallel processing
let (results_tx, results_rx) = unbounded::<(PathBuf, Result<()>)>();

// In worker thread
let _ = results_tx.send((path.clone(), result));

// After processing
let mut summary = ConversionSummary::default();
while let Ok((path, res)) = results_rx.try_recv() {
    match res {
        Ok(()) => summary.succeeded += 1,
        Err(e) => summary.failed.push((path, e.to_string())),
    }
}
summary.print_summary();
std::process::exit(summary.exit_code());
```

---

## Common Pitfalls

### 1. Forgetting to Use Constants

**Wrong**:
```rust
let default_threads = (total_cpus as f64 * 0.75).ceil() as usize;  // Magic number!
```

**Correct**:
```rust
use crate::constants::DEFAULT_CPU_RATIO;
let default_threads = (total_cpus as f64 * DEFAULT_CPU_RATIO).ceil() as usize;
```

### 2. Not Using the FFmpeg Builder

**Wrong**:
```rust
let mut cmd = Command::new("ffmpeg");
cmd.args(["-y", "-hide_banner", "-hwaccel", "cuda", ...]);  // Manual construction
```

**Correct**:
```rust
let cmd = FfmpegCommandBuilder::new(input, output)
    .hwaccel(HwAccel::Cuda)
    .video_codec(VideoCodec::Av1Nvenc)
    .build();
```

### 3. Ignoring ConversionSummary

**Wrong**:
```rust
// Silently succeed even if some files failed
Ok(())
```

**Correct**:
```rust
summary.print_summary();
if summary.exit_code() != 0 {
    return Err(anyhow::anyhow!("Some conversions failed"));
}
Ok(())
```

### 4. Using .unwrap() Instead of Safe Alternatives

**Wrong**:
```rust
spinner_pool.lock().unwrap()
```

**Correct**:
```rust
spinner_pool.lock().ok()  // Returns Option
// or
spinner_pool.lock().map_err(|_| MyError::LockPoisoned)?  // Returns Result
```

---

## Code Style Guidelines

### Import Organization (Updated)

```rust
// 1. Crate-level imports (including constants)
use crate::constants::{DEFAULT_CPU_RATIO, CHANNEL_BUFFER_MULTIPLIER};
use crate::image::{ImageArgs, ImageError, ConversionSummary};
use crate::video::builder::{FfmpegCommandBuilder, HwAccel, VideoCodec};
use crate::walker::Walker;
use crate::{CpuControl, PathUtil, SHUTDOWN};

// 2. External crate imports
use crossbeam_channel::{bounded, Receiver, Sender};
use image::DynamicImage;
use rayon::prelude::*;

// 3. Standard library imports
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
```

---

## Appendix: Quick Reference

### Command Examples

```bash
# Image conversion
media-forge image output/ -s input/ -f avif -q 72 --speed 4 -j 8

# Archive creation
media-forge archive -s ./comics --recursive --cleanup --dry-run

# Video encoding
media-forge video output/ -s input/ --cq 28 --preset p6 -j 2

# Quality analysis
media-forge simulacra original.png distorted.avif
media-forge vmaf original.mp4 encoded.mkv --duration 60
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success (all operations completed) |
| 1 | Partial failure (some operations failed) |
| 130 | Interrupted (Ctrl+C) |

### Environment Requirements

- **Rust**: 2024 edition (nightly features: `let_chains`)
- **System Libraries**: `libdav1d-dev` for AVIF support
- **FFmpeg**: Required for video commands, must include:
  - NVENC support for encoding
  - libvmaf for quality analysis
- **NVIDIA GPU**: For hardware-accelerated video encoding

---

## Changelog

### 2026-01-10 (Post-Refactoring)
- ✅ Added `src/constants.rs` - Extracted all magic numbers
- ✅ Added `src/video/builder.rs` - FFmpeg command builder with fluent API
- ✅ Added `tests/image_convert.rs` - Integration tests for image conversion
- ✅ Added `tests/archive_create.rs` - Integration tests for archive creation
- ✅ Fixed unsafe `.expect()` in `main.rs` - Now uses `try_set_handler()?`
- ✅ Fixed mutex `.unwrap()` calls in `convert.rs` - Now uses safe patterns
- ✅ Added `ConversionSummary` for error aggregation
- ✅ Added 4 unit tests to `walker.rs`
- ✅ Added 3 unit tests to `video/builder.rs`
- ✅ Added 3 unit tests to `lib.rs` (CpuControl, PathUtil)
- ✅ Added 2 unit tests to `video/mod.rs` (regex parsing)

### Initial Analysis
- Documented architecture, patterns, and refactoring targets
- Identified 23 unsafe unwrap instances
- Proposed testing strategy and code style guidelines

---

*Last updated: 2026-01-10*
*Generated for LLM-assisted refactoring of media-forge codebase*
