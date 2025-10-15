# Pex – Plex EPG Explorer

Pex is a desktop viewer for the Plex Electronic Program Guide (EPG). It reads
Plex’s DVR SQLite database, assembles a poster wall of upcoming airings,
highlights the titles you already own, and layers on rich metadata such as
channel badges, HD/SD hints, and on-demand IMDb ratings.

---

## Table of Contents
- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [Getting Started](#getting-started)
- [Configuration Reference](#configuration-reference)
- [Data & Cache Locations](#data--cache-locations)
- [Typical Workflows](#typical-workflows)
- [Troubleshooting & Diagnostics](#troubleshooting--diagnostics)
- [Development Notes](#development-notes)
- [Legal](#legal)

---

## Overview

Highlights:

- 🚀 **Fast start-up** – poster and channel artwork are cached and uploaded on
demand, keeping the UI responsive even on large guides.
- 🗂️ **Powerful filtering** – search, day-range slicing, HD-only toggle, channel
and genre selectors, plus multiple sort orders.
- 📚 **Owned library awareness** – incremental scans of your movie folders feed
owned/HD badges and “recorded on” timestamps directly into the grid.
- 🎨 **Detail-rich panels** – long-title scroller with copy button, channel
badges, optional IMDb ratings, and formatted descriptions.
- 🧰 **Operator controls** – quick refresh/clear actions for poster, ffprobe,
and owned caches as well as worker tuning knobs.

Pex runs on Windows, macOS, and Linux (including WSL) using
[egui/eframe](https://github.com/emilk/egui) for the native UI.

---

## Prerequisites

### Plex data access
- Plex DVR/EPG must be enabled so that `tv.plex.providers.epg.cloud-*.db` (and
the accompanying WAL/SHM files) exist on disk.
- Common locations:
  - **Windows:** `%LOCALAPPDATA%\Plex Media Server\Plug-in Support\Databases`
  - **Linux (service install):** `/var/lib/plexmediaserver/Library/Application Support/Plex Media Server/Plug-in Support/Databases`
  - **Docker:** bind the Plex config volume; copy the database out via the host.
- Pex expects a working copy in `db/plex_epg.db`. You can supply it manually or
point `plex_db_source` at the live Plex database so Pex pulls in a fresh copy
once per day.

### Toolchain
- Rust toolchain **1.74 or newer** (`rustup toolchain install stable`)
- Cargo (bundled with Rust)
- `ffprobe` from FFmpeg on your PATH (or set `ffprobe_cmd` in the config)
- Git (to clone the repository)

### External services
- **OMDb API key** (optional, but recommended). Supplying a personal key avoids
the heavy throttling on the public demo key when fetching IMDb ratings.

---

## Getting Started

1. **Clone and bootstrap**
   ```bash
   git clone https://github.com/AnicetusCer/pex.git
   cd pex
   cp config.example.json config.json
   ```

2. **Populate the database folder**
   - Copy your Plex EPG SQLite file into `db/plex_epg.db`, *or*
   - Set `plex_db_source` in `config.json` to the path of Plex’s live DB so Pex
     can maintain `db/plex_epg.db` automatically.

3. **Edit `config.json`** (see the [Configuration Reference](#configuration-reference)).

4. **Build and run the app**
   ```bash
   cargo run --release
   ```
   The first launch will
   - copy the Plex database if `plex_db_source` is set,
   - scan your owned library roots,
   - warm the ffprobe cache, and
   - start poster prefetching.

   Subsequent runs reuse the cached data, so they reach the UI much faster.

---

## Configuration Reference

Pex reads `config.json` from the repository root. All keys are optional unless
otherwise stated; absent keys fall back to reasonable defaults.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `library_roots` | array of strings | `[]` | Absolute paths scanned for owned movie files. Multiple paths are supported. |
| `plex_db_source` | string or `null` | `null` | When set, Pex copies the live Plex EPG SQLite file into `db/plex_epg.db` no more than once every 24 hours. Leave unset if you manage `db/plex_epg.db` yourself. |
| `cache_dir` | string or `null` | `.pex_cache` | Root folder for poster caches, owned-manifest data, UI prefs, and ffprobe cache. |
| `ffprobe_cmd` | string or `null` | `ffprobe` | Override the ffprobe executable (useful on Windows/WSL when ffprobe is not on PATH). |
| `omdb_api_key` | string or `null` | demo key | Personal OMDb API key for IMDb ratings. Leave blank to use the public key (heavy rate limiting). |
| `log_level` | string | `info` | Controls tracing output (`trace`, `debug`, `info`, `warn`, `error`). |

Example configuration:

```json
{
  "library_roots": [
    "D:/Libraries/Movies",
    "\\\
as\Archive\Films"
  ],
  "plex_db_source": "\\\
as\PlexConfig\Databases\\tv.plex.providers.epg.cloud.db",
  "cache_dir": ".pex_cache",
  "ffprobe_cmd": "C:/Tools/ffmpeg/bin/ffprobe.exe",
  "omdb_api_key": "YOUR-OMDB-KEY",
  "log_level": "info"
}
```

> Tip: on Linux paths use forward slashes; on Windows double any backslashes in
JSON strings or switch to forward slashes.

---

## Data & Cache Locations

- `db/plex_epg.db` — working copy of the Plex EPG database (plus WAL/SHM files
  created on demand).
- `.pex_cache/` — posters, channel icons, owned-manifest, ffprobe cache, and UI
  preference files. Poster images older than 14 days are pruned automatically
  on startup.
- `db/` and `.pex_cache/` are created automatically; you can relocate caches by
  setting `cache_dir` in the config file.

---

## Typical Workflows

### First run (cold start)
1. Copy or configure access to the Plex DB.
2. Populate `config.json` with your library roots and ffprobe/OMDb settings.
3. Launch with `cargo run --release`.
4. Let the initial owned scan and poster prefetch finish (progress appears in
the status bar). Large libraries may take several minutes.

### Daily usage
- Launch the app; the UI resumes where you left off.
- If `plex_db_source` is set, Pex checks once per day whether the Plex DB needs
  copying.
- Owned and HD badges stay up-to-date thanks to incremental scanning and the
  ffprobe cache.

### Keeping the owned manifest fresh
- Use **Advanced ▸ Refresh owned scan** after adding/removing many files.
- Use **Advanced ▸ Clear owned cache** only when you want a full rescan from
  scratch (e.g., after reorganising folder structures).

### Poster & ffprobe maintenance
- **Advanced ▸ Refresh poster cache** removes zero-byte / partial downloads and
enforces the poster limit.
- **Advanced ▸ Refresh ffprobe cache** re-validates stored resolutions without
  touching the filesystem.
- **Advanced ▸ Clear ffprobe cache** clears the cache and immediately rebuilds
  HD flags; useful after upgrading ffprobe.

### Building a portable package
See [`make_portable/README.md`](./make_portable/README.md) for instructions on
producing a self-contained ZIP using the provided PowerShell/Bash scripts.

---

## Troubleshooting & Diagnostics

- **Status bar stuck on Stage 2/4 (DB copy):** verify the `plex_db_source` path
  is reachable and that you have permission to read it.
- **Owned scan never completes:** check the Advanced panel log for the last
  processed directory; ensure all library roots are accessible and contain only
  media files you expect.
- **Missing posters:** confirm outbound network access to the artwork URLs and
  that the cache directory is writable. Pex prunes posters older than 14 days
  automatically, so re-open the app after a while to fetch fresh artwork.
- **HD badge looks wrong:** run **Advanced ▸ Refresh ffprobe cache** and ensure
  `ffprobe_cmd` points to a recent FFmpeg build.
- **Logging:** set `log_level` to `debug` and relaunch to capture richer logs in
  the console.

---

## Development Notes

- Format the code with `cargo fmt`.
- Run tests with `cargo test`.
- Optional: `cargo clippy --all-targets -- -D warnings` (requires the Clippy
  component).
- DB inspection helper: `cargo run --bin db_explorer metadata_items 10`.

---

## Legal

- Licensed under the [PEX Attribution License (PAL)](./LICENSE).
- This application uses `ffprobe` from the FFmpeg project. If you redistribute
  the binary with ffprobe bundled, include FFmpeg’s required notices.
- IMDb ratings are fetched via the OMDb API; please respect their usage terms
  and attribution requirements.
- Plex is a registered trademark of Plex, Inc. This project is an independent
  client that reads the Plex DVR database.
- Additional third-party notices are listed in [`NOTICE`](./NOTICE).
