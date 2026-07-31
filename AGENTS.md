# Media Forge agent instructions

Media Forge is a focused Rust CLI for still-image conversion, still-image
quality comparison, and CBZ archive creation. Treat this file as the repository
entry point for coding agents. `CLAUDE.md` is an alias to this file.

## Scope boundary

- Keep this repository image-only.
- Supported product areas are AVIF/WebP conversion, SSIMULACRA2 comparison for
  still images, ZIP/CBZ image extraction, and CBZ creation.
- Do not add video probing, transcoding, quality targeting, VMAF, FFmpeg, GPU
  video metrics, or video codec integrations here.
- Video work belongs in
  [media-agents](https://github.com/delfianto/media-agents). Make video changes
  in that repository instead of adding a bridge or compatibility layer here.
- Video inputs encountered by image conversion must remain ignored, not copied
  into the output tree.

## Repository map

```text
src/main.rs            CLI definition and command routing
src/lib.rs             shared process state and public modules
src/image/convert.rs   image discovery, conversion, and archive extraction
src/image/quality.rs   SSIMULACRA2 comparison
src/image/archive.rs   CBZ creation and guarded cleanup
src/walker.rs          filesystem traversal
src/ui.rs              progress presentation
tests/                 CLI and end-to-end behavior
README.md              user-facing commands and project boundary
justfile               canonical local workflows
```

The public CLI has exactly three command families: `image`, `simulacra`, and
`archive`. Keep help text, the desktop entry, tests, and README synchronized
when changing that surface.

## Working rules

- Preserve source directory structure and modification times unless an
  explicit option says otherwise.
- Existing AVIF/WebP and animated GIF/APNG inputs are pass-through image files;
  JPEG, PNG, TIFF, and BMP are convertible inputs.
- Treat archive cleanup as destructive. Preserve dry-run and confirmation
  guards, and verify the created archive before removing source data.
- Keep cancellation behavior responsive and avoid leaving partially written
  output presented as a successful conversion.
- Prefer focused dependencies. Do not introduce a system multimedia framework
  for image-only behavior.
- Add or update integration tests for user-visible CLI, filesystem, conversion,
  or cleanup behavior.
- Keep `README.md` current when commands, formats, prerequisites, or safety
  behavior change.

## Validation

Run the repository gate before declaring a change complete:

```bash
just full-gate
```

For release-sensitive changes, also run:

```bash
cargo build --release
```

`just build` is different: it enables native-CPU optimization and requires UPX.
Use it when validating that deployment path specifically.
