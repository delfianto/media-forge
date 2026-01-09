# Media-Forge

A high-performance CLI tool for batch media conversion on Linux. Convert images to modern compression formats (AVIF, WebP) with minimal quality loss, encode videos to AV1 using NVIDIA CUDA acceleration, and create comic book archives (CBZ) from image folders.

## Features

- **Image Conversion**: Batch convert images to AVIF or WebP formats with configurable quality and speed
- **Video Encoding**: Hardware-accelerated AV1 encoding using NVIDIA NVENC via FFmpeg
- **Archive Creation**: Create CBZ comic book archives with automatic page numbering and natural sorting
- **Parallel Processing**: Multi-threaded processing with intelligent resource management
- **Progress Tracking**: Real-time progress bars with per-task and overall progress indicators
- **Archive Support**: Extract and convert images directly from ZIP/CBZ archives

## Requirements

### System Requirements

- Linux operating system
- Rust 2024 edition (for building from source)

### For Video Encoding

- NVIDIA GPU with NVENC support (GTX 10-series or newer)
- NVIDIA CUDA Toolkit
- FFmpeg compiled with NVENC support (`av1_nvenc` codec)

### For Image Conversion

- No special hardware required (CPU-based encoding)

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/yourusername/media-forge.git
cd media-forge

# Build release binary
cargo build --release

# Install to system (requires sudo)
just install
```

### Using Just (Recommended)

```bash
# Build release binary
just build

# Install to /usr/local/bin
just install

# Uninstall
just uninstall
```

## Usage

Media-Forge provides three subcommands for different media types:

### Image Conversion

Convert images to AVIF or WebP format:

```bash
# Basic usage - convert all images to AVIF
media-forge image ./output

# Convert to WebP with custom quality
media-forge image ./output --source ./photos --format webp --quality 85

# High-quality archival encoding (slower, best compression)
media-forge image ./output --speed 1 --quality 85

# Fast encoding for previews (faster, acceptable quality)
media-forge image ./output --speed 6 --quality 70

# Process with specific thread count
media-forge image ./output --jobs 8

# Deep scan into subdirectories
media-forge image ./output --depth 5
```

**Options:**

| Option        | Short | Default   | Description                                                                    |
| ------------- | ----- | --------- | ------------------------------------------------------------------------------ |
| `destination` | -     | Required  | Output directory for converted images                                          |
| `--source`    | `-s`  | `.`       | Source directory to scan for images                                            |
| `--format`    | `-f`  | `avif`    | Output format: `avif` or `webp`                                                |
| `--quality`   | `-q`  | `72`      | Quality (0-100, higher = better). Default balances quality & file size.        |
| `--speed`     | -     | `4`       | AVIF encoding speed (0-10). Lower = slower but smaller files. See guide below. |
| `--depth`     | -     | `2`       | Directory recursion depth                                                      |
| `--jobs`      | `-j`  | 75% cores | Number of parallel threads                                                     |
| `--no-mtime`  | -     | false     | Don't preserve original modification times                                     |

> **📖 See [Understanding Encoding Parameters](#understanding-encoding-parameters)** for detailed explanations of quality and speed settings, including why the defaults were chosen and when to change them.

**Supported Input Formats:** JPG, JPEG, PNG, WebP, TIFF, BMP, ZIP, CBZ

### Video Encoding

Encode videos to AV1 using NVIDIA hardware acceleration:

```bash
# Basic usage - encode all videos to AV1
media-forge video ./output

# Custom quality (lower = better)
media-forge video ./output --cq 24

# Faster encoding with lower quality
media-forge video ./output --preset p7 --cq 32

# Highest quality encoding
media-forge video ./output --preset p1 --cq 20

# Output as MP4 instead of MKV
media-forge video ./output --ext mp4

# Process multiple files concurrently (requires GPU memory)
media-forge video ./output --jobs 2
```

**Options:**

| Option        | Short | Default  | Description                                     |
| ------------- | ----- | -------- | ----------------------------------------------- |
| `destination` | -     | Required | Output directory for encoded videos             |
| `--source`    | `-s`  | `.`      | Source directory to scan for videos             |
| `--cq`        | -     | `28`     | Constant quality (1-51, lower = better quality) |
| `--preset`    | -     | `p6`     | NVENC preset: `p1`-`p7` (p1 = slowest/best)     |
| `--ext`       | -     | `mkv`    | Output container: `mkv` or `mp4`                |
| `--depth`     | -     | `2`      | Directory recursion depth                       |
| `--jobs`      | `-j`  | `1`      | Concurrent encoding jobs                        |

> **📖 See [Understanding Encoding Parameters](#understanding-encoding-parameters)** for detailed explanations of CQ and preset settings, including quality tradeoffs and when to use different values.

**Supported Input Formats:** MP4, MKV, MOV, AVI, TS, M4V

**Note:** Videos already encoded in AV1 are automatically skipped.

### Archive Creation

Create CBZ comic book archives from image folders:

```bash
# Basic usage - archive folders in current directory
media-forge archive

# Specify output directory
media-forge archive ./archives --source ./manga

# Recursively process nested folders
media-forge archive --recursive

# Preview what would be archived (dry run)
media-forge archive --dry-run

# Preview cleanup operation (RECOMMENDED - always do this first!)
media-forge archive --dry-run --cleanup

# Archive and delete source folders (requires confirmation)
media-forge archive --cleanup

# Skip confirmation prompt (for automation only)
media-forge archive --cleanup --force
```

**Options:**

| Option        | Short | Default        | Description                                                        |
| ------------- | ----- | -------------- | ------------------------------------------------------------------ |
| `destination` | -     | Same as source | Output directory for CBZ files                                     |
| `--source`    | `-s`  | `.`            | Source directory containing image folders                          |
| `--recursive` | -     | false          | Scan subdirectories for image folders                              |
| `--cleanup`   | -     | false          | Delete source folders after archiving (requires confirmation)      |
| `--dry-run`   | `-n`  | false          | Show what would be done without executing                          |
| `--force`     | -     | false          | Skip confirmation prompt for --cleanup (automation only)           |
| `--jobs`      | `-j`  | 75% cores      | Number of parallel threads                                         |

**Archive Structure:**

Files are automatically renamed for proper ordering in comic readers:

- First image: `000_cover.ext`
- Middle images: `page_001.ext`, `page_002.ext`, ...
- Last image: `999_back.ext`

**⚠️ Cleanup Safety:**

When using `--cleanup`, the program implements multiple safety checks to prevent accidental data loss:

1. **Interactive Confirmation**: You must type `DELETE` (all caps) to confirm deletion
2. **Folder Preview**: Shows up to 10 folders that will be deleted before asking for confirmation
3. **Clear Warnings**: Color-coded warnings (yellow/red) highlight the permanent nature of deletion
4. **Dry-Run First**: Always recommended to run with `--dry-run --cleanup` first to preview operations
5. **Archive Verification**: Verifies archive integrity before deleting source folders
6. **Force Flag**: `--force` skips confirmation (use only in automated scripts with extreme caution)

**Recommended Workflow:**

```bash
# Step 1: Preview what will be archived and deleted
media-forge archive --dry-run --cleanup

# Step 2: Review the output carefully

# Step 3: If everything looks correct, run with confirmation
media-forge archive --cleanup
# (You will be prompted to type 'DELETE' to confirm)
```

## Understanding Encoding Parameters

### AVIF Parameters (Image Encoding)

#### Quality (`--quality`, default: 72)

The quality parameter (0-100) controls the visual fidelity vs file size tradeoff:

- **Scale**: 0 (lowest quality) to 100 (highest quality)
- **Default (72)**: Chosen as the optimal balance point where:
    - Visual quality is perceptually close to lossless for most content
    - File sizes remain reasonable (typically 40-60% smaller than equivalent JPEG)
    - Roughly equivalent to JPEG quality 85-90 due to AVIF's superior compression algorithm
    - Quality loss is imperceptible to most viewers on most displays

**Guidelines:**

- **85-95**: Near-transparent quality, archival use. Recommended for photography where preserving fine details is critical. Diminishing returns above 85.
- **70-85**: High quality, general use. Best for web content, digital art, and most photographs.
- **60-70**: Medium quality. Acceptable for thumbnails or when file size is more important than quality.
- **< 60**: Visible artifacts appear. Only use for maximum compression where quality is not critical.

#### Speed (`--speed`, default: 4)

The speed parameter (0-10) controls encoding time vs compression efficiency:

- **Scale**: 0 (slowest) to 10 (fastest)
- **Default (4)**: Chosen as the sweet spot where:
    - Encoding time is reasonable for batch processing (typically 1-3 seconds per image)
    - Compression efficiency is near-optimal (within 5-10% of best possible)
    - Quality-per-byte ratio is maximized for most content types
    - Suitable for production use without excessive wait times

**How Speed Works:**

- Lower values = MORE encoding effort → SMALLER files with BETTER compression
- Higher values = LESS encoding effort → LARGER files with WORSE compression
- Speed affects compression efficiency, NOT visual quality at the same quality setting

**Guidelines:**

- **0-2**: Very slow (10-60 seconds per image). Best possible compression. Use for archival where file size is critical and time doesn't matter. Produces files 10-25% smaller than speed 4.
- **3-5**: Moderate speed (1-5 seconds per image). Good compression efficiency. Recommended range for most use cases. Speed 4 is the optimal balance.
- **6-8**: Fast (0.5-2 seconds per image). Acceptable compression. Use for quick conversions or previews where time is more important than optimal file size. Files are 15-30% larger than speed 4.
- **9-10**: ⚠️ **VERY FAST but POOR COMPRESSION** (< 0.5 seconds per image). Files can be 40-60% larger than speed 4 with identical visual quality. Only use for testing or when encoding time is absolutely critical.

**WARNING**: Never use speed 9-10 for production unless you understand you're sacrificing significant compression efficiency (larger files) for minimal time savings. These settings exist for debugging and testing, not production use.

### AV1 Parameters (Video Encoding)

#### Constant Quality (`--cq`, default: 28)

The CQ parameter (1-51) controls video quality using a "lower is better" scale:

- **Scale**: 1 (highest quality) to 51 (lowest quality)
- **Default (28)**: Chosen as the balanced setting where:
    - Visual quality is excellent for most content (high definition streaming quality)
    - File sizes are 50-70% smaller than original H.264/H.265 sources
    - Encoding time is reasonable with NVENC hardware acceleration
    - Suitable for archiving content you want to keep at high quality

**Guidelines:**

- **18-22**: Near-transparent quality. Visually indistinguishable from source in most cases. Use for archival of irreplaceable content. Files are larger but still smaller than original codecs.
- **23-28**: High quality. Excellent for general use. CQ 28 is the recommended default for most users who want good quality at reasonable file sizes.
- **29-35**: Medium quality. Acceptable for streaming or when storage space is limited. Some quality loss visible in complex scenes.
- **36+**: Low quality. Visible artifacts in most content. Only use for maximum compression when quality doesn't matter.

**Note**: AV1's perceptual quality at CQ 28 is roughly equivalent to H.264 at CRF 23 or H.265 at CRF 26, but with 40-50% better compression efficiency.

#### NVENC Preset (`--preset`, default: p6)

The preset (p1-p7) controls the encoder's speed/quality tradeoff:

- **Scale**: p1 (slowest, best) to p7 (fastest, lowest)
- **Default (p6)**: Chosen because:
    - Hardware acceleration makes it very fast (often faster than realtime)
    - Quality is acceptable for most use cases
    - GPU memory usage is moderate, allowing parallel processing if desired
    - Good balance between throughput and quality

**Guidelines:**

- **p1-p3**: Slow, best quality. Use for final archival encodes of important content. Quality improvement over p6 is typically 2-5% at the same CQ.
- **p4-p5**: Balanced. Good quality with reasonable speed. Consider if you have time and want marginally better quality.
- **p6**: Fast, good quality. Recommended default. Excellent speed with acceptable quality loss.
- **p7**: ⚠️ **Fastest but lower quality**. Quality loss is noticeable (5-10% worse than p6). Only use for quick tests or previews.

**Note**: NVENC presets have less quality impact than software encoders because the hardware encoder is optimized for speed. The difference between p1 and p6 is smaller than you might expect (~2-5% quality difference).

## Quality Guidelines Summary

### Image Quality (AVIF)

| Quality          | Speed           | Use Case              | File Size  | Encoding Time       |
| ---------------- | --------------- | --------------------- | ---------- | ------------------- |
| 85-95            | 1-2             | Archival, photography | Large      | Very slow (30-60s)  |
| **72 (default)** | **4 (default)** | **General use, web**  | **Medium** | **Moderate (1-3s)** |
| 60-70            | 6-8             | Previews, thumbnails  | Small      | Fast (0.5-2s)       |
| < 60             | 8-10            | Maximum compression   | Very small | Very fast (<0.5s)   |

### Video Quality (AV1)

| CQ Value            | Preset           | Use Case            | Quality          | File Size  |
| ------------------- | ---------------- | ------------------- | ---------------- | ---------- |
| 18-22               | p1-p3            | Archival            | Near-transparent | Large      |
| **23-28 (default)** | **p6 (default)** | **General use**     | **Excellent**    | **Medium** |
| 29-35               | p6-p7            | Streaming           | Good             | Small      |
| 36+                 | p7               | Maximum compression | Acceptable       | Very small |

## Performance Tips

1. **Use release builds**: Debug builds are 10-100x slower. Always use `cargo build --release` for production.
2. **SSD storage**: Significantly improves I/O-bound operations, especially for batch processing.
3. **Thread count**: Default (75% cores) works well for most systems. Increase for I/O-bound workloads, decrease for memory-constrained systems.
4. **Video jobs**: Keep low (1-2) to avoid GPU memory exhaustion. Each concurrent video encode uses significant VRAM.
5. **AVIF speed tradeoff**: Lower speed values (0-3) produce SMALLER files but take LONGER to encode. Higher speed values (6-10) produce LARGER files but encode FASTER. The default (4) is the optimal balance. Only change if you understand the tradeoff.
6. **Batch processing**: Process large batches together rather than one-by-one to amortize scanning overhead.

## Project Structure

```
media-forge/
├── crates/
│   ├── media_forge/    # CLI entry point
│   ├── mf_core/        # Shared utilities
│   ├── mf_image/       # Image conversion
│   ├── mf_archive/     # Archive creation
│   └── mf_video/       # Video encoding
├── Cargo.toml          # Workspace configuration
├── Justfile            # Build recipes
└── README.md           # This file
```

## Development

### Building

```bash
# Debug build
cargo build

# Release build (recommended)
cargo build --release

# Run without installing
cargo run --release -- image ./output
```

### Code Quality

```bash
# Run all checks
just check

# Individual checks
cargo check          # Type checking
cargo clippy         # Linting
cargo fmt --check    # Format checking
cargo test           # Tests
```

### Release Optimizations

The release profile is configured for maximum performance:

```toml
[profile.release]
lto = "fat"           # Link-time optimization
codegen-units = 1     # Better optimization
panic = "abort"       # Smaller binary
strip = true          # Remove debug symbols
```

## Troubleshooting

### FFmpeg not found

Ensure FFmpeg is installed and in your PATH:

```bash
ffmpeg -version
ffprobe -version
```

### NVENC not available

1. Verify NVIDIA driver is installed: `nvidia-smi`
2. Check FFmpeg NVENC support: `ffmpeg -encoders | grep nvenc`
3. Ensure your GPU supports NVENC (GTX 10-series or newer)

### Out of GPU memory

Reduce concurrent video jobs:

```bash
media-forge video ./output --jobs 1
```

### Slow image encoding

1. **Check build type**: Ensure you're using `--release` build (debug builds are 10-100x slower)
2. **Understand speed tradeoffs**: Higher speed values (6-8) encode faster but produce larger files (15-30% bigger). Only increase speed if encoding time is more important than file size.
3. **Reduce thread count if memory-constrained**: `--jobs 4` (fewer threads = less memory usage)
4. **Profile bottleneck**: Use `time` command to check if CPU or I/O is the bottleneck

```bash
# Fast encoding with acceptable quality (larger files)
media-forge image ./output --speed 6 --quality 70

# Verify you're using release build
cargo build --release
./target/release/media-forge image ./output
```
