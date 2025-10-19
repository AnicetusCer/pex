# Pex – Plex EPG Explorer

Pex is a desktop viewer for the Plex Electronic Program Guide (EPG). It reads
Plex’s DVR SQLite database, assembles a poster wall of upcoming airings,
highlights the titles you already own, and layers on rich metadata such as
channel badges, HD/SD hints, and on-demand IMDb ratings.

---

### Why I built it

- This started as a personal project: each week I sift through upcoming TV movies to decide what to record.
- The stock Plex web UI felt too barebones for that workflow, so I wanted a richer experience for anyone in the Plex community who still enjoys browsing broadcast schedules.
- While the app concentrates on TV films, the bundled SQLite explorer tools make it easy for others to tweak or extend it for different Plex data.
- Developed primarily on Windows 11 with WSL Fedora 42; both environments are exercised regularly. macOS hasn't been tested first-hand, but the Rust/egui stack produces native binaries for both x86_64 and ARM64, so it should run wherever those architectures are supported (Intel/AMD PCs or Apple Silicon).

---

## Downloads & Releases

- Releases live on the [GitHub Releases page](https://github.com/AnicetusCer/pex/releases); each tagged version includes portable bundles.
- Use the scripts in `make_portable/` to produce fresh zips before a release; the outputs land in `make_portable/dist/` ready for upload.
- The interactive helper `pwsh ./release.ps1 -Version 1.2.3` walks through build, tagging, pushing, and optional `gh release create` steps.
- Generated binaries are not checked into git; attach them to the corresponding GitHub release instead.

---

## Table of Contents
- [Downloads & Releases](#downloads--releases)
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
- ?? **Owned library awareness** - mirrored Plex library data (with optional filesystem scanning) feeds owned/HD badges and "recorded on" timestamps directly into the grid.
- 🎯 **DVR awareness** – scheduled recordings show a red *REC* badge and detail call-out pulled from the Plex library database.
- 🎨 **Detail-rich panels** – long-title scroller with copy button, channel
badges, optional IMDb ratings, and formatted descriptions.
- 🧰 **Operator controls** – quick refresh/clear actions for poster, ffprobe,
and owned caches as well as worker tuning knobs.

Pex runs on Windows, macOS, and Linux (including WSL) using
[egui/eframe](https://github.com/emilk/egui) for the native UI.

---

## Repository Layout

- `src/`
  - `main.rs` / `lib.rs` – launch the egui app and wire tracing.
  - `config.rs` – parses `config.json`, owns `OwnedSourceKind`, and exposes helper paths for the copied databases.
  - `app/`
    - `mod.rs` – central application state, message pump, advanced actions, and egui integration.
    - `prep.rs` – copies the Plex databases (daily freshness), queries poster rows, and emits `PrepMsg`.
    - `prefetch.rs` / `gfx.rs` – background workers + GPU upload helpers for poster textures.
    - `cache.rs` / `prefs.rs` – cache directory helpers, poster/file pruning, persisted UI preferences.
    - `owned/` – filesystem and Plex-library scanners that build `owned_manifest.json` and HD sidecars.
    - `scheduled.rs` – loads scheduled DVR entries from `media_grabs`, `media_subscriptions`, and `metadata_subscription_desired_items`.
    - `detail.rs`, `filters.rs`, `types.rs`, `utils.rs` – UI panels, filtering & sorting logic, shared structs, and formatting helpers.
    - `ui/` – splash/grid/top bar egui widgets.
  - `assets/` – embedded icon and other compile-time resources.
  - `bin/` – optional CLI entry-points used during development.
- `epg_explorer_tool/`
  - `db_explorer.rs` – CLI for poking at the EPG SQLite.
  - `library_db_explorer.rs` – companion CLI for the Plex library database (owned + DVR state).
  - `*_ai_primer.yaml` – primers that document how the helpers should be extended.
- `make_portable/` – scripts and template config for packaging a portable build.
- `db/` – working copies of the Plex databases (populated on first run).
- `.pex_cache/` – generated cache directory (posters, owned manifests, ffprobe cache, UI prefs).
- `config.example.json` – starter configuration; copy to `config.json` for local runs.

---

## Prerequisites

### Plex data access
- Plex DVR/EPG must be enabled so that `tv.plex.providers.epg.cloud-*.db` (and
the accompanying WAL/SHM files) exist on disk.
- Common locations:
  - **Windows:** `%LOCALAPPDATA%\Plex Media Server\Plug-in Support\Databases`
  - **Linux (service install):** `/var/lib/plexmediaserver/Library/Application Support/Plex Media Server/Plug-in Support/Databases`
  - **Docker:** bind the Plex config volume; copy the database out via the host.
- Pex expects a working copy in `db/plex_epg.db`. You can supply it manually or point `plex_epg_db_source` at the live Plex database so Pex pulls in a fresh copy once per day.

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
   - Set `plex_epg_db_source` in `config.json` to the path of Plex’s live DB so Pex
     can maintain `db/plex_epg.db` automatically.
   - Optionally set `plex_library_db_source` to the path of Plex’s library
     database so Pex keeps `db/plex_library.db` in sync for owned detection
     experiments.
   - Choose `owned_source`: keep the default and switch to `plex_library` for
     the fastest start-up, or set it to `filesystem` to crawl your local folders.
     The filesystem mode walks every movie directory and expects files named
     `Title (Year).ext`, so large libraries can take quite a while on the first run.

3. **Edit `config.json`** (see the [Configuration Reference](#configuration-reference)).

4. **Build and run the app**
   ```bash
   cargo run --release
   ```
   The first launch will
   - copy the Plex database if `plex_epg_db_source` is set,
   - copy the Plex library database if `plex_library_db_source` is set,
   - load owned titles using the configured `owned_source` (filesystem scans may
     take a long time on first run),
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
| `owned_source` | string | `filesystem` | Choose how Pex determines owned titles: `plex_library` (recommended) reads from Plex's `com.plexapp.plugins.library.db`; `filesystem` crawls the folders in `library_roots`, which can take a long time on large collections and expects files named `Title (Year).ext`. |
| `plex_epg_db_source` | string or `null` | `null` | When set, Pex copies the live Plex EPG SQLite file into `db/plex_epg.db` no more than once every 24 hours. Leave unset if you manage `db/plex_epg.db` yourself. |
| `plex_library_db_source` | string or `null` | `null` | When set, Pex copies Plex’s library SQLite file into `db/plex_library.db` on the same 24-hour freshness cadence. Leave unset if you manage `db/plex_library.db` yourself. |
| `cache_dir` | string or `null` | `.pex_cache` | Root folder for poster caches, owned-manifest data, UI prefs, and ffprobe cache. |
| `ffprobe_cmd` | string or `null` | `ffprobe` | Override the ffprobe executable (useful on Windows/WSL when ffprobe is not on PATH). |
| `omdb_api_key` | string or `null` | demo key | Personal OMDb API key for IMDb ratings. Leave blank to use the public key (heavy rate limiting). |
| `log_level` | string | `info` | Controls tracing output (`trace`, `debug`, `info`, `warn`, `error`). |

Example configuration:

```json
{
  "library_roots": [
    "D:/Libraries/Movies",
    "\\\\nas\\Archive\\Films"
  ],
  "plex_epg_db_source": "\\\\nas\\PlexConfig\\Databases\\tv.plex.providers.epg.cloud.db",
  "plex_library_db_source": "\\\\ds\\PlexMediaServer\\AppData\\Plex Media Server\\Plug-in Support\\Databases\\com.plexapp.plugins.library.db",
  "owned_source": "filesystem",
  "cache_dir": ".pex_cache",
  "ffprobe_cmd": "C:/Tools/ffmpeg/bin/ffprobe.exe",
  "omdb_api_key": "YOUR-OMDB-KEY",
  "log_level": "info"
}
```

> Tip: on Linux paths use forward slashes; on Windows double any backslashes in
JSON strings or switch to forward slashes.

### Owned detection modes

- `owned_source = "filesystem"` (default)  
  Walks every directory listed in `library_roots`, normalises filenames such as
  `Movie Title (2024).mkv`, and caches the mapping in `owned_manifest.json`.
  The first run can take a long time on large NAS shares, but subsequent runs
  only rescan folders whose timestamp changed.
- `owned_source = "plex_library"`  
  Reads owned titles from the Plex library database (`db/plex_library.db`)
  instead of crawling the filesystem. This is the fastest option once
  `plex_library_db_source` is configured.

### Database copy settings

- `plex_epg_db_source` – path to `tv.plex.providers.epg.cloud*.db` on the Plex
  server. Pex copies it into `db/plex_epg.db` (refreshing at most once every 24 h).
- `plex_library_db_source` – optional path to
  `com.plexapp.plugins.library.db`. When present, Pex clones it into
  `db/plex_library.db` and uses it for owned detection and DVR badges.

You can locate these files by:

| Platform | Default location |
| --- | --- |
| **Windows (desktop)** | `%LOCALAPPDATA%\Plex Media Server\Plug-in Support\Databases\` |
| **Windows (service mode)** | `%PROGRAMDATA%\Plex Media Server\Plug-in Support\Databases\` |
| **Linux** | `/var/lib/plexmediaserver/Library/Application Support/Plex Media Server/Plug-in Support/Databases/` |
| **Synology** | `/var/packages/Plex Media Server/target/Plex Media Server/Plug-in Support/Databases/` |
| **Docker** | Whatever host path you bind to `/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/` |

Copy both `.db` files plus their `-wal`/`-shm` companions (if present) while Plex is stopped, or let Pex copy from the live path on each start.

### Other useful keys

- `library_roots` – one or more absolute paths containing your movie files. Use
  forward slashes on every OS (e.g., `D:/Media/Movies`, `/mnt/nas/movies`).
- `cache_dir` – move the poster/manifest/ffprobe cache elsewhere; relative paths are resolved relative to the repo root.
- `ffprobe_cmd` – explicit path to `ffprobe` when it is not on `PATH`. Accepts either `"C:/Tools/ffmpeg/bin/ffprobe.exe"` or `/usr/bin/ffprobe`.
- `omdb_api_key` – replace the bundled demo key (`thewdb`) with your personal OMDb key to avoid rate limits.
- `log_level` – override the default tracing verbosity (`trace` → most verbose).

### Environment variables

- `PEX_DISABLE_PREFETCH=1` – skip poster downloads (useful when testing offline modes).
- `RUST_LOG=info` (or `debug`) – surface prep/owned/scheduled traces in the terminal.

---

## Data & Cache Locations

- `db/plex_epg.db` — working copy of the Plex EPG database (plus WAL/SHM files
  created on demand).
- `db/plex_library.db` — optional working copy of Plex’s library database,
  copied when `plex_library_db_source` is configured.
- DVR metadata (`media_grabs`, `media_subscriptions`, `metadata_subscription_desired_items`) is read from `db/plex_library.db` to drive the *REC* badge and owned detection.
- `.pex_cache/` — posters, channel icons, owned-manifest, ffprobe cache, and UI
  preference files. Poster images older than 14 days are pruned automatically
  on startup.
- `db/` and `.pex_cache/` are created automatically; you can relocate caches by
  setting `cache_dir` in the config file.

---

## Typical Workflows

### First run (cold start)
1. Copy or configure access to the Plex EPG DB (and optionally the library DB).
2. Populate `config.json` with your library roots and ffprobe/OMDb settings.
3. Launch with `cargo run --release`.
4. Let the initial owned scan and poster prefetch finish (progress appears in
the status bar). Large libraries may take several minutes.

### Daily usage
- Launch the app; the UI resumes where you left off.
- If `plex_epg_db_source` or `plex_library_db_source` is set, Pex checks once per
  day whether the respective database copy needs refreshing.
- Scheduled recordings sync automatically after poster prep; queued movies show a red *REC* badge in the grid and detail panel.
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

- **Status bar stuck on Stage 2/4 (DB copy):** verify the `plex_epg_db_source` and
  `plex_library_db_source` paths are reachable and readable.
- **Owned scan never completes:** check the Advanced panel log for the last
  processed directory; ensure all library roots are accessible and contain only
  media files you expect.
- **REC badge missing:** confirm `plex_library_db_source` is configured, the copied `db/plex_library.db` contains up-to-date `media_subscriptions` rows, and you restarted after scheduling the recording.
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
- DB inspection helpers:
  - `cargo run --bin db_explorer metadata_items 10`
  - `cargo run --bin library_db_explorer -- --tables`

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


