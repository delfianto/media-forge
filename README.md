# Media Forge

A focused Linux CLI for batch-converting still images to AVIF or WebP,
measuring image quality with SSIMULACRA2, and building CBZ archives.

Media Forge is intentionally image-only. Video analysis, quality targeting,
and transcoding belong in
[media-agents](https://github.com/delfianto/media-agents), not this project.

## Features

- Batch AVIF and WebP conversion with configurable quality and speed
- Native SSIMULACRA2 comparison for still images
- ZIP/CBZ image extraction and conversion
- CBZ creation with natural page ordering
- Parallel processing with progress reporting
- Directory structure and modification-time preservation

## Requirements

- Linux
- A current stable Rust toolchain when building from source
- `clang`, `nasm`, `pkg-config`, and the dav1d development library
- No GPU or FFmpeg dependency

AVIF encoding is CPU- and memory-intensive and uses modern SIMD instructions
where available. Hardware instability or an unstable overclock can become
visible during large conversion jobs.

## Installation

```bash
git clone https://github.com/delfianto/media-forge.git
cd media-forge
cargo build --release
```

The included `justfile` can build a native-CPU optimized binary and install it
to `~/.local/bin`. Its release path also requires UPX:

```bash
just build
just install
```

## Image conversion

```bash
# Convert images in the current directory to AVIF
media-forge image ./output

# Convert a photo directory to WebP
media-forge image ./output \
  --source ./photos \
  --format webp \
  --quality 85

# High-compression AVIF
media-forge image ./output --speed 1 --quality 85

# Faster conversion
media-forge image ./output --speed 6 --quality 70

# Control concurrency and scanning depth
media-forge image ./output --jobs 8 --depth 5
```

| Option | Default | Description |
| --- | --- | --- |
| `destination` | required | Output file or directory |
| `--source`, `-s` | `.` | Source file or directory; repeatable |
| `--format`, `-f` | `avif` | `avif` or `webp` |
| `--quality`, `-q` | `80` | Quality from 0–100 |
| `--speed` | `4` | AVIF speed from 0–10; lower compresses more slowly |
| `--depth` | `2` | Maximum recursion depth |
| `--jobs`, `-j` | 75% of cores | Worker count |
| `--no-mtime` | false | Do not preserve source modification times |
| `--overwrite`, `-o` | false | Replace existing output files |
| `--report`, `-r` | false | Write a post-conversion SSIMULACRA2 CSV report |

Convertible inputs: JPEG, PNG, TIFF, and BMP. Existing AVIF/WebP files and
animated GIF/APNG files are copied without re-encoding. ZIP and CBZ inputs are
extracted and their supported images are converted.

Video files are intentionally ignored; Media Forge no longer performs or
proxies video operations. Use
[media-agents](https://github.com/delfianto/media-agents) for video workflows.

### AVIF settings

Quality controls fidelity and file size:

- `85–95`: near-transparent archival or photographic output
- `70–85`: high-quality general use
- `60–70`: smaller previews and thumbnails
- Below `60`: maximum compression where visible artifacts are acceptable

Speed controls encoding effort, not the requested quality:

- `0–2`: slowest, best compression efficiency
- `3–5`: balanced; the default is `4`
- `6–8`: faster with larger output
- `9–10`: fastest, primarily useful for previews or testing

## Image quality

Compare a distorted image with its reference using SSIMULACRA2:

```bash
media-forge simulacra original.png encoded.avif
```

Both images must have matching dimensions.

## CBZ archive creation

```bash
# Archive image folders in the current directory
media-forge archive

# Choose source and destination
media-forge archive ./archives --source ./manga

# Include nested folders
media-forge archive --recursive

# Preview archive creation and cleanup
media-forge archive --dry-run --cleanup

# Execute cleanup after confirmation
media-forge archive --cleanup
```

Cleanup is destructive and therefore guarded by confirmation. Use
`--dry-run --cleanup` first. The archive is verified before its source folder
is removed.

## Development

```bash
just full-gate
just build --debug
```

The full local gate runs formatting checks, Clippy with warnings denied, and
the test suite. The equivalent Cargo commands are:

```bash
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

Use `cargo build --release` for a portable release build. `just build` adds
`-C target-cpu=native` and UPX compression, so its output is intended for the
machine class on which it was built.

## Safety

Media Forge is provided without warranty. Keep backups and validate a sample
of every important conversion batch before deleting source material.
