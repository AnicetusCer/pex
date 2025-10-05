# Pex

Pex is an egui desktop app that inspects a Plex EPG database, prepares poster assets, and highlights titles you already own.

## Prerequisites

- **ffprobe** (ships with [FFmpeg](https://ffmpeg.org/download.html)). The owned library scan uses it to read real video dimensions so it can accurately mark HD holdings.
  - Windows: install the static FFmpeg build and add the folder containing `ffprobe.exe` to your `PATH`.
  - macOS: `brew install ffmpeg` (or another package manager of choice).
  - Linux: install the ffmpeg package from your distro (for example `sudo apt install ffmpeg`).

If `ffprobe` is missing the app falls back to filename heuristics and warns in the status area.

Verify the binary is available by running `ffprobe -version` in a terminal before launching Pex.

## Running

```
cargo run --release
```

\nFirst launch after adding new media rebuilds .pex_cache/owned_all.txt, .pex_cache/owned_hd.txt, and .pex_cache/owned_manifest.json. Subsequent runs reuse the manifest and only rescan directories whose timestamps change, keeping startup snappy even with large libraries.\n



