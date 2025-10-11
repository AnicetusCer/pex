# Pex – Plex EPG Explorer

Pex is a fast, desktop viewer for the Plex Electronic Program Guide (EPG). It scans the Plex TV database, builds a poster wall, highlights titles you already own, and lets you explore schedules with rich detail overlays, channel badges, and on-demand IMDb lookups.

---

## Feature Highlights

- 🚀 **Snappy startup** – pre-caches channel icons and poster thumbnails, loading them lazily when needed.
- 🗂️ **Smart filtering** – day range, full-text search, HD-only toggle, channel & genre include lists, plus owned/HD badges.
- 📚 **Owned library awareness** – incremental rescans detect library changes, show “Owned file recorded” dates, and keep SD-vs-HD indicators accurate.
- 🎨 **Rich details panel** – channel badges, long-title scroller with clipboard button, on-demand IMDb rating fetch, and formatted descriptions.
- 🧰 **Advanced controls** – gentle refresh buttons for poster, ffprobe, and owned caches, plus the original hard-reset options when you truly need a clean slate.
- 🪟 **Custom app icon & slick UI** – ships with the included `PEX.ico` and an egui-based layout tuned for keyboard and mouse navigation.

---

## Quick Start

1. **Install prerequisites**
   - Rust toolchain (1.74+) and Cargo
   - `ffprobe` from the FFmpeg suite (add it to your `PATH` or set `ffprobe_cmd` in the config)
   - Plex running with DVR/EPG data so the `plex_epg.db` SQLite file is available

2. **Clone and configure**
   ```bash
   git clone https://github.com/your-account/pex.git
   cd pex
   cp config.example.json config.json
   ```
   Edit `config.json` with your paths and library directories.

3. **Run the app**
   ```bash
   cargo run --release
   ```
   The first launch primes the caches. Subsequent runs reuse the cached posters and owned-manifest data for a much faster startup.

---

## Configuration

Pex reads `config.json` from the project root. The most important keys are:

| Key | Description |
| --- | --- |
| `plex_db_local` | Path to the Plex EPG SQLite file (e.g., `plex_epg.db`). |
| `plex_db_source` | Optional upstream DB path; if set, Pex copies it locally once per day. |
| `library_roots` | Array of library directories to scan for owned titles. |
| `cache_dir` | Root folder for posters, icons, manifests, and preferences. |
| `ffprobe_cmd` | Custom path to `ffprobe` (useful on WSL). |
| `omdb_api_key` | Personal OMDb API key. If omitted, Pex falls back to the demo key. |
| `poster_cache_max_files` | Optional limit for poster thumbnails (defaults to 1500). |

See [`config.example.json`](./config.example.json) for a complete template with comments.

---

## UI Overview

- **Top bar controls** – choose day ranges, search titles, toggle HD-only, open channel/genre filters, change sorting, tweak poster size, and manage owned highlighting.
- **Poster grid** – grouped by day, showing title/year, humanised channel with HD badge, and airing time in UTC. Owned titles appear dimmed or hidden depending on your toggles.
- **Detail panel** – expands on selection to show the poster, channel logo, long-title scroller, optional IMDb rating button, owned status (with recorded date), description, and genre list.
- **Advanced panel** – exposes worker tuning, cache refresh/clear buttons, and UI preference backup/restore utilities.

---

## Advanced Controls

Navigate to **Advanced…** (top bar) for:

- Poster cache refresh/prune or full clear
- Owned library refresh (non-destructive) or full reset
- ffprobe cache refresh or clear
- UI preference backups (`.pex_cache/ui_prefs_backup_*.txt`) and one-click restore
- Status readouts for important config paths (Plex DB location, OMDb key state)

---

## Tips & Troubleshooting

- **Slow first load?** The initial run downloads channel icons and poster thumbnails. Subsequent launches reuse the cache.
- **Missing posters?** Ensure `poster_cache_max_files` is large enough and that the HTTP requests aren’t blocked by a firewall.
- **Owned scan stuck?** Check the Advanced view for log messages; you can refresh the scan without clearing metadata.
- **IMDb rate limiting?** Supply your own `omdb_api_key` to avoid demo-key throttling.

---

## Development Notes

- Run the app with `cargo run --release` for realistic performance (debug builds are noticeably slower).
- The codebase follows `rustfmt`; run `cargo fmt` if the toolchain component is installed.
- Lint with `cargo clippy --all-targets --all-features -- -W clippy::all -W clippy::nursery`.
- DB exploration utility: `cargo run --bin db_explorer metadata_items 5`.

---

## License

This project is licensed under the terms specified by the repository owner. See `LICENSE` if provided.

