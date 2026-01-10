# AGENTS.md - LLM-Assisted Refactoring Guide for Media-Forge

> **Purpose**: This document provides comprehensive guidance for LLM agents performing refactoring, bug fixes, feature additions, and code improvements on the media-forge codebase.

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
9. [Critical Refactoring Targets](#critical-refactoring-targets)
10. [Testing Strategy](#testing-strategy)
11. [Common Pitfalls](#common-pitfalls)
12. [Code Style Guidelines](#code-style-guidelines)

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
│  │  - Task creation │  │  - CBZ creation  │  │  - FFmpeg spawning       │    │
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
│                              UI Layer (ui.rs)                                 │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────────┐   │
│  │ Progress Styles │  │ Spinner Styles  │  │ Multi-Progress Management  │   │
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
├── Cargo.toml              # Rust 2024 edition, dependencies
├── src/
│   ├── main.rs             # CLI entry point, command routing (92 lines)
│   ├── lib.rs              # Core utilities, constants, ProcessManager (201 lines)
│   ├── walker.rs           # File/archive scanning abstraction (181 lines)
│   ├── ui.rs               # Progress bar styles and factories (61 lines)
│   ├── image/
│   │   ├── mod.rs          # ImageError, Args structs, public API (208 lines)
│   │   ├── convert.rs      # Image conversion pipeline (562 lines)
│   │   ├── archive.rs      # CBZ archive creation (361 lines)
│   │   ├── quality.rs      # SSIMULACRA2 scoring (~100 lines)
│   │   └── report.rs       # Quality report generation (379 lines)
│   └── video/
│       ├── mod.rs          # VideoError, Args structs, metadata (231 lines)
│       ├── encode.rs       # FFmpeg video encoding (292 lines)
│       └── quality.rs      # VMAF quality analysis (268 lines)
└── tests/                  # (Currently empty - needs population)
```

### Lines of Code by Module

| Module | Lines | Purpose | Test Coverage |
|--------|-------|---------|---------------|
| `src/image/convert.rs` | 562 | Image conversion pipeline | ❌ None |
| `src/image/report.rs` | 379 | Quality report generation | ❌ None |
| `src/image/archive.rs` | 361 | CBZ archive creation | ❌ None |
| `src/video/encode.rs` | 292 | FFmpeg video encoding | ❌ None |
| `src/video/quality.rs` | 268 | VMAF analysis | ❌ None |
| `src/video/mod.rs` | 231 | Video error types, metadata | ❌ None |
| `src/image/mod.rs` | 208 | Image error types, args | ❌ None |
| `src/lib.rs` | 201 | Core utilities | ✅ 5 tests |
| `src/walker.rs` | 181 | File scanning | ❌ None |
| `src/main.rs` | 92 | CLI entry | ❌ None |
| `src/ui.rs` | 61 | Progress styles | ❌ None |

**Total**: ~2,836 lines with 5 unit tests (0.18% test coverage)

---

## Module Deep-Dive

### `src/main.rs` - CLI Entry Point

**Responsibilities**:
- MiMalloc global allocator initialization
- Clap-based argument parsing
- Ctrl+C handler registration
- Command routing to appropriate modules

**Key Code Patterns**:
```rust
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;  // Performance optimization

ctrlc::set_handler(move || {
    ProcessManager::kill_all();      // Graceful shutdown
    std::process::exit(130);
}).expect("Error setting Ctrl-C handler");  // ⚠️ UNSAFE EXPECT
```

**Refactoring Notes**:
- The `expect()` on line 81 should use proper error propagation
- Consider extracting command routing to a separate function

---

### `src/lib.rs` - Core Utilities

**Key Components**:

1. **Global Constants**:
   ```rust
   pub const IMAGE_EXTENSIONS: &[&str] = &["avif", "webp", "jpg", "jpeg", "png", "tiff", "bmp"];
   pub const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "cbz"];
   pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "ts", "m4v", "mpv", "webm"];
   ```

2. **ProcessManager** (lines 35-64):
   - Thread-safe process PID tracking
   - SIGKILL-based cleanup on shutdown
   - Uses `Lazy<Mutex<HashSet<u32>>>` for storage

3. **PathUtil** (lines 68-97):
   - Path canonicalization
   - Destination resolution
   - Skip checking (existence + non-empty)

4. **CpuControl** (lines 100-117):
   - Thread count calculation
   - Default: 75% of available cores
   - Clamp: 1 to 150% of cores

5. **Naming** (lines 120-160):
   - Unicode-aware filename truncation
   - Cover image heuristic detection

**Magic Numbers to Extract**:
```rust
// Line 109: Hardcoded ratios
let default_threads = (total_cpus as f64 * 0.75).ceil() as usize;  // → DEFAULT_CPU_RATIO
let max_limit = (total_cpus as f64 * 1.5).ceil() as usize;         // → MAX_CPU_RATIO
```

---

### `src/walker.rs` - File Scanning

**Core Abstraction**:
```rust
pub enum MediaSource {
    Filesystem(PathBuf),           // Direct file on disk
    Archive {
        archive_path: PathBuf,     // Path to ZIP/CBZ
        entry_name: String,        // Path inside archive
    },
}

pub struct Asset {
    pub path: PathBuf,             // Physical file path
    pub source: MediaSource,       // Origin metadata
}
```

**Scan Methods**:
- `scan_flat()` - Returns flat `Vec<Asset>`
- `scan_grouped()` - Returns `HashMap<PathBuf, Vec<Asset>>` by parent dir
- `scan_with_progress()` - Includes progress bar updates

**Refactoring Opportunities**:
1. Add `FileSystem` trait for testability
2. Extract archive scanning to separate method
3. Add error collection instead of silent skipping

---

### `src/image/convert.rs` - Image Conversion Pipeline

**Pipeline Stages**:
```
1. collect_tasks()      → Parallel asset analysis, task generation
2. process_tasks()      → Parallel task execution with progress
3. process_single_task() → Individual image encoding
4. encode_avif()        → AVIF encoding via ravif
```

**Task Types**:
```rust
pub(crate) enum TaskType {
    File,                          // Standalone image file
    Archive { internal_path: String }, // Image inside ZIP/CBZ
    Copy,                          // Non-image file passthrough
}
```

**Worker Thread Model** (lines 357-379):
```rust
fn spawn_workers<'a>(
    scope: &'a std::thread::Scope<'a, '_>,
    rx: Receiver<WorkItem>,
    args: Arc<ImageArgs>,
    spinner_pool: Arc<Mutex<Vec<ProgressBar>>>,
    num_threads: usize,
) {
    for _ in 0..num_threads {
        scope.spawn(move || {
            while let Ok(item) = rx.recv() {
                if SHUTDOWN.load(Ordering::SeqCst) { return; }
                process_work_item(item, &args, &spinner_pool);
            }
        });
    }
}
```

**Critical Unsafe Code** (lines 551-553):
```rust
// AVIF encoding with unsafe pointer cast
let rgba_pixels = unsafe {
    std::slice::from_raw_parts(pixels.as_ptr() as *const ravif::RGBA8, pixels.len() / 4)
};
```
⚠️ **Refactoring Target**: This can be replaced with safe alternatives from the `rgb` crate.

**Unsafe Unwraps** (HIGH PRIORITY):
- Line 128, 180, 217, 343, 401, 417: `spinner_pool.lock().unwrap()`
- Line 245, 263: `path.file_name().unwrap()`

---

### `src/image/archive.rs` - CBZ Archive Creation

**Key Features**:
- Natural sorting via `natord` crate
- Archive verification before deletion
- Dry-run mode support
- Explicit "DELETE" confirmation for cleanup

**Verification Logic** (simplified):
```rust
fn verify_archive(archive_path: &Path, expected_count: usize) -> Result<()> {
    let file = fs::File::open(archive_path)?;
    let archive = ZipArchive::new(file)?;
    if archive.len() != expected_count {
        return Err(ImageError::VerificationFailed(archive_path.to_path_buf()));
    }
    Ok(())
}
```

---

### `src/video/encode.rs` - FFmpeg Video Encoding

**FFmpeg Command Construction** (lines 207-237):
```rust
let mut cmd = Command::new("ffmpeg");
cmd.args([
    "-y",                           // Overwrite output
    "-hide_banner",
    "-loglevel", "error",
    "-stats",                       // Enable progress output
    "-hwaccel", "cuda",             // NVIDIA hardware acceleration
    "-hwaccel_output_format", "cuda",
    "-i", src_str,
    "-c:v", "av1_nvenc",            // AV1 NVENC encoder
    "-preset", &args.preset,        // p1-p7
    "-tune", "hq",                  // High quality tuning
    "-cq", &args.cq.to_string(),    // Constant quality (1-51)
    "-c:a", "copy",                 // Copy audio stream
]);
```

**Progress Parsing** (lines 264-270):
```rust
// Regex: r"time=(\d+):(\d+):(\d+\.\d+)"
if let Some(caps) = FFMPEG_PROGRESS_RE.captures(&line) {
    let h: u64 = caps[1].parse().unwrap_or(0);
    let m: u64 = caps[2].parse().unwrap_or(0);
    let s: f64 = caps[3].parse().unwrap_or(0.0);
    let seconds = (h * 3600 + m * 60) as f64 + s;
    pb.set_position(seconds as u64);
}
```

**Refactoring Opportunity**: Extract FFmpeg command building to a `FfmpegCommandBuilder` struct.

---

### `src/video/quality.rs` - VMAF Analysis

**FFmpeg VMAF Filter**:
```rust
// Constructs: [0:v]scale=...[ref];[1:v]scale=...[dist];[ref][dist]libvmaf=...
let filter = format!(
    "[0:v]scale=-1:{}:flags=bicubic[ref];[1:v]scale=-1:{}:flags=bicubic[dist];[ref][dist]libvmaf=log_fmt=json:log_path={}:n_threads={}:n_subsample={}",
    scale, scale, log_path, threads, subsample
);
```

**Unsafe Unwraps** (MEDIUM PRIORITY):
- Line 135, 148: `path.to_str().unwrap()`

---

## Key Patterns and Conventions

### 1. Error Type Pattern

Each module defines its own error type using `thiserror`:

```rust
#[derive(Error, Debug)]
pub enum ImageError {
    #[error("Source path does not exist: {0:?}")]
    SourceNotFound(PathBuf),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),  // Auto-conversion via From trait
    // ...
}

pub type Result<T> = std::result::Result<T, ImageError>;
```

### 2. Progress Bar Pattern

```rust
let mp = MultiProgress::new();              // Container for multiple bars
let pb_main = mp.add(ProgressBar::new(n));  // Main progress bar
pb_main.set_style(ui::main_bar_style());    // Apply consistent styling

// In worker threads:
pb_main.inc(1);                             // Thread-safe increment
```

### 3. Parallel Processing Pattern

```rust
use rayon::prelude::*;

let results: Vec<_> = items
    .par_iter()                              // Parallel iterator
    .map(|item| process(item))               // Map in parallel
    .collect();                              // Collect results
```

### 4. Channel-Based Work Distribution

```rust
let (tx, rx) = bounded::<WorkItem>(num_threads * 2);  // Bounded channel

// Producer thread
for task in tasks {
    tx.send(WorkItem { task, ... })?;
}
drop(tx);  // Signal completion

// Worker threads
while let Ok(item) = rx.recv() {
    process(item);
}
```

### 5. Graceful Shutdown Pattern

```rust
// Check before starting work
if SHUTDOWN.load(Ordering::SeqCst) {
    return;
}

// Check during long operations
while reader.read_until(b'\r', &mut buffer)? > 0 {
    if SHUTDOWN.load(Ordering::SeqCst) {
        let _ = child.kill();
        break;
    }
    // ... process
}
```

---

## Error Handling

### Error Type Hierarchy

```
anyhow::Result<()>          ← Main entry points return this
    ↑
    │ map_err()
    │
ImageError / VideoError     ← Domain-specific errors
    ↑
    │ #[from] or map_err()
    │
std::io::Error              ← Low-level errors
zip::result::ZipError
image::ImageError
serde_json::Error
```

### Error Propagation Patterns

**Good Pattern** (used throughout):
```rust
let source_path = fs::canonicalize(&args.source)
    .map_err(|_| ImageError::SourceNotFound(args.source.clone()))?;
```

**Problematic Pattern** (needs refactoring):
```rust
// Silent error swallowing in parallel contexts
if let Err(e) = process_single_task(&item, args) {
    eprintln!("Error processing {}: {}", name_display, e);
}
// Processing continues, no error aggregation
```

### Recommended Error Aggregation

```rust
pub struct BatchResult {
    pub succeeded: usize,
    pub failed: usize,
    pub errors: Vec<(PathBuf, ImageError)>,
}

impl BatchResult {
    pub fn exit_code(&self) -> i32 {
        if self.failed > 0 { 1 } else { 0 }
    }

    pub fn summary(&self) -> String {
        format!("{} succeeded, {} failed", self.succeeded, self.failed)
    }
}
```

---

## Concurrency Model

### Thread Pool Configuration

| Context | Thread Count | Rationale |
|---------|--------------|-----------|
| Image conversion | 75% of cores | CPU-bound, leave headroom |
| Video encoding | User-specified (default 1) | GPU memory limited |
| Task collection | min(cores/2, 8) | I/O bound scanning |

### Synchronization Primitives

| Primitive | Location | Purpose |
|-----------|----------|---------|
| `AtomicBool` | `lib.rs:20` | Global shutdown flag |
| `Mutex<HashSet<u32>>` | `lib.rs:32` | Active process PIDs |
| `Mutex<Vec<ProgressBar>>` | `convert.rs:103` | Spinner pool |
| `AtomicUsize` | `convert.rs:108` | File counter |
| `crossbeam_channel::bounded` | `convert.rs:335` | Work distribution |
| `crossbeam_channel::unbounded` | `convert.rs:440` | Completion signals |

### Potential Race Conditions

1. **Spinner pool contention** (convert.rs):
   - Multiple threads compete for spinners
   - Mitigated by pool size matching thread count

2. **Process kill during encoding** (encode.rs):
   - Ctrl+C can trigger while FFmpeg is running
   - Mitigated by ProcessManager tracking

---

## Dependencies

### Core Dependencies

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `clap` | 4.5 | CLI argument parsing | derive feature |
| `rayon` | 1.11 | Data parallelism | Thread pool management |
| `crossbeam-channel` | 0.5.15 | MPMC channels | Work distribution |
| `indicatif` | 0.18 | Progress bars | rayon feature |
| `thiserror` | 2.0 | Error derive macros | |
| `anyhow` | 1.0 | Error context | Top-level errors |

### Image Processing

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `image` | 0.25 | Image loading/saving | avif-native, webp features |
| `ravif` | 0.12 | AVIF encoding | High-performance encoder |
| `ssimulacra2` | 0.5.1 | Image quality metric | |

### File Handling

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `walkdir` | 2.5 | Directory traversal | |
| `zip` | 2.4 | ZIP/CBZ handling | |
| `natord` | 1.0 | Natural sorting | Comic page ordering |

### Utilities

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `mimalloc` | 0.1 | Memory allocator | v3 feature |
| `once_cell` | 1.21 | Lazy statics | Regex caching |
| `regex` | 1.12 | Pattern matching | FFmpeg output parsing |
| `serde` / `serde_json` | 1.0 | JSON handling | ffprobe output |
| `ctrlc` | 3.4 | Signal handling | Graceful shutdown |

### Dev Dependencies

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `tempfile` | 3.10 | Temporary directories | **Currently unused** |

---

## Critical Refactoring Targets

### Priority 1: Unsafe Unwrap Removal (23 instances)

| Location | Pattern | Risk | Suggested Fix |
|----------|---------|------|---------------|
| `main.rs:81` | `.expect("Error setting Ctrl-C handler")` | CRITICAL | Return `Result` from main setup |
| `convert.rs:128,180,217,343,401,417` | `spinner_pool.lock().unwrap()` | HIGH | Use `lock().ok()` or propagate error |
| `convert.rs:245,263` | `file_name().unwrap()` | MEDIUM | Use `ok_or_else()` |
| `video/quality.rs:135,148` | `to_str().unwrap()` | MEDIUM | Use `ok_or(VideoError::InvalidPath)` |
| `ui.rs:11,18,25,34,42,50` | `.expect("Invalid template")` | LOW | Safe - hardcoded strings |

**Example Fix**:
```rust
// Before
let pool = spinner_pool.lock().unwrap();

// After
let pool = spinner_pool.lock().map_err(|_| ImageError::LockPoisoned)?;

// Or if in non-Result context
let Some(pool) = spinner_pool.lock().ok() else { return; };
```

### Priority 2: Add Comprehensive Tests

**Test Categories Needed**:

1. **Unit Tests** (~50 tests estimated):
   ```rust
   // src/lib.rs tests
   #[test] fn test_cpu_control_default_threads() { ... }
   #[test] fn test_cpu_control_clamp_limits() { ... }
   #[test] fn test_path_util_resolve_absolute() { ... }
   #[test] fn test_path_util_resolve_relative() { ... }
   #[test] fn test_path_util_should_skip_existing() { ... }
   ```

2. **Integration Tests** (~20 tests estimated):
   ```rust
   // tests/image_convert.rs
   #[test] fn test_jpeg_to_avif_basic() { ... }
   #[test] fn test_png_to_webp_with_alpha() { ... }
   #[test] fn test_cbz_extraction_and_convert() { ... }
   #[test] fn test_overwrite_existing_file() { ... }
   #[test] fn test_preserve_mtime() { ... }
   ```

3. **Mock-Based Tests**:
   ```rust
   // Trait for filesystem abstraction
   trait FileSystem {
       fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
       fn read_file(&self, path: &Path) -> io::Result<Vec<u8>>;
       fn write_file(&self, path: &Path, data: &[u8]) -> io::Result<()>;
   }

   struct MockFileSystem {
       files: HashMap<PathBuf, Vec<u8>>,
   }
   ```

### Priority 3: Error Aggregation

**Current Behavior**:
- Errors logged via `eprintln!()`
- Processing continues
- Exit code always 0

**Desired Behavior**:
```rust
pub struct ConversionSummary {
    pub total: usize,
    pub succeeded: usize,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
}

impl ConversionSummary {
    pub fn print_summary(&self) {
        println!("Processed {} files:", self.total);
        println!("  ✓ {} succeeded", self.succeeded);
        println!("  → {} skipped", self.skipped);
        if !self.failed.is_empty() {
            println!("  ✗ {} failed:", self.failed.len());
            for (path, error) in &self.failed {
                println!("    - {:?}: {}", path, error);
            }
        }
    }

    pub fn exit_code(&self) -> i32 {
        if self.failed.is_empty() { 0 } else { 1 }
    }
}
```

### Priority 4: Extract Magic Numbers

**Create `src/constants.rs`**:
```rust
//! Application-wide constants

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
pub const VMAF_ERROR_LOG: &str = "/tmp/media-forge-vmaf-error.log";

/// Progress bar tick interval
pub const SPINNER_TICK_MS: u64 = 100;
```

### Priority 5: FFmpeg Command Builder

```rust
pub struct FfmpegCommand {
    input: PathBuf,
    output: PathBuf,
    hwaccel: Option<HwAccel>,
    video_codec: VideoCodec,
    audio_codec: AudioCodec,
    preset: String,
    quality: u8,
}

pub enum HwAccel {
    Cuda,
    Vaapi,
    None,
}

pub enum VideoCodec {
    Av1Nvenc,
    Hevc,
    H264,
    Copy,
}

impl FfmpegCommand {
    pub fn builder() -> FfmpegCommandBuilder { ... }

    pub fn into_command(self) -> Command {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y").arg("-hide_banner");

        if let Some(hwaccel) = &self.hwaccel {
            match hwaccel {
                HwAccel::Cuda => {
                    cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
                }
                // ...
            }
        }

        cmd.args(["-i", self.input.to_str().unwrap()]);
        // ... build rest of command
        cmd
    }
}
```

---

## Testing Strategy

### Test File Structure

```
tests/
├── common/
│   └── mod.rs              # Shared test utilities
├── integration/
│   ├── image_convert.rs    # Image conversion tests
│   ├── archive_create.rs   # CBZ creation tests
│   └── video_encode.rs     # Video encoding tests (requires FFmpeg)
└── unit/
    ├── walker_test.rs      # File scanning tests
    ├── naming_test.rs      # Filename utility tests
    └── path_util_test.rs   # Path resolution tests
```

### Test Utilities

```rust
// tests/common/mod.rs
use std::path::PathBuf;
use tempfile::TempDir;

pub struct TestFixture {
    pub temp_dir: TempDir,
    pub source_dir: PathBuf,
    pub dest_dir: PathBuf,
}

impl TestFixture {
    pub fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let source_dir = temp_dir.path().join("source");
        let dest_dir = temp_dir.path().join("dest");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        Self { temp_dir, source_dir, dest_dir }
    }

    pub fn create_test_image(&self, name: &str, width: u32, height: u32) -> PathBuf {
        let path = self.source_dir.join(name);
        let img = image::RgbImage::new(width, height);
        img.save(&path).unwrap();
        path
    }
}
```

### Example Integration Test

```rust
// tests/integration/image_convert.rs
use media_forge::image::{ImageArgs, run};
use crate::common::TestFixture;

#[test]
fn test_jpeg_to_avif_preserves_dimensions() {
    let fixture = TestFixture::new();
    let source = fixture.create_test_image("test.jpg", 1920, 1080);

    let args = ImageArgs {
        source: fixture.source_dir.clone(),
        destination: fixture.dest_dir.clone(),
        format: "avif".to_string(),
        quality: 72,
        speed: 4,
        depth: 1,
        jobs: Some(1),
        no_mtime: true,
        overwrite: false,
        report: false,
    };

    run(args).unwrap();

    let output = fixture.dest_dir.join("test.avif");
    assert!(output.exists());

    let img = image::open(&output).unwrap();
    assert_eq!(img.dimensions(), (1920, 1080));
}
```

---

## Common Pitfalls

### 1. Forgetting to Check SHUTDOWN Flag

**Wrong**:
```rust
for task in tasks {
    process(task);  // Won't stop on Ctrl+C
}
```

**Correct**:
```rust
for task in tasks {
    if SHUTDOWN.load(Ordering::SeqCst) {
        break;
    }
    process(task);
}
```

### 2. Not Registering Child Processes

**Wrong**:
```rust
let child = Command::new("ffmpeg").spawn()?;
// If Ctrl+C happens, ffmpeg keeps running
```

**Correct**:
```rust
let child = Command::new("ffmpeg").spawn()?;
ProcessManager::register(child.id());
// ... use child ...
ProcessManager::unregister(child.id());
```

### 3. Swallowing Errors in Parallel Contexts

**Wrong**:
```rust
items.par_iter().for_each(|item| {
    let _ = process(item);  // Errors lost
});
```

**Better**:
```rust
let errors: Vec<_> = items
    .par_iter()
    .filter_map(|item| process(item).err())
    .collect();
if !errors.is_empty() {
    // Handle or report errors
}
```

### 4. Using unwrap() on User-Provided Paths

**Wrong**:
```rust
let name = path.file_name().unwrap().to_str().unwrap();
```

**Correct**:
```rust
let name = path
    .file_name()
    .and_then(|n| n.to_str())
    .ok_or_else(|| ImageError::InvalidFilename(path.to_path_buf()))?;
```

### 5. Hardcoding Progress Bar Templates

**Wrong**:
```rust
pb.set_style(ProgressStyle::default_bar()
    .template("{bar:40} {pos}/{len}")
    .unwrap());  // Duplicated everywhere
```

**Correct**:
```rust
pb.set_style(ui::main_bar_style());  // Centralized in ui.rs
```

---

## Code Style Guidelines

### Naming Conventions

| Type | Convention | Example |
|------|------------|---------|
| Functions | snake_case | `process_single_task` |
| Types | PascalCase | `ImageError`, `VideoTask` |
| Constants | SCREAMING_SNAKE | `IMAGE_EXTENSIONS` |
| Module-private items | Leading underscore optional | `_internal_helper` |
| Type aliases | PascalCase | `Result<T>` |

### Import Organization

```rust
// 1. Crate-level imports
use crate::image::{ImageArgs, ImageError};
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

### Documentation Standards

```rust
/// Converts a batch of images to the specified format.
///
/// # Arguments
///
/// * `args` - Configuration for the conversion process
///
/// # Returns
///
/// Returns `Ok(())` on successful completion, or an error if:
/// - Source path doesn't exist
/// - Thread pool creation fails
/// - All conversions fail
///
/// # Example
///
/// ```no_run
/// use media_forge::image::{ImageArgs, run};
///
/// let args = ImageArgs { ... };
/// run(args)?;
/// ```
pub fn run(args: ImageArgs) -> anyhow::Result<()> {
```

### Error Message Style

```rust
// Include context in error messages
#[error("Failed to canonicalize path {0:?}: {1}")]
CanonicalizationError(PathBuf, #[source] std::io::Error),

// Use Debug formatting for paths
#[error("Source path does not exist: {0:?}")]  // Shows full path with quotes
SourceNotFound(PathBuf),

// Provide actionable information
#[error("FFmpeg not found. Install FFmpeg with NVENC support (av1_nvenc codec)")]
FfmpegNotFound,
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
| 0 | Success |
| 1 | General error |
| 130 | Interrupted (Ctrl+C) |

### Environment Requirements

- **Rust**: 2024 edition (nightly features: `let_chains`)
- **FFmpeg**: Required for video commands, must include:
  - NVENC support for encoding
  - libvmaf for quality analysis
- **NVIDIA GPU**: For hardware-accelerated video encoding

---

*Last updated: 2026-01-10*
*Generated for LLM-assisted refactoring of media-forge codebase*
