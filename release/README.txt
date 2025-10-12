Pex Portable Package
=====================

Contents
--------
- `pex.exe` (copied from `target/release/pex.exe` after building)
- `config.json` (shipping template – edit before running; contains `REPLACE_ME` placeholders)
- `README.txt` (this file)

Quick Start
-----------
1. Build the binary: `cargo build --release`
2. Package automatically: `pwsh release/package.ps1 -Zip`
   - or manually copy `target/release/pex.exe` into this folder.
3. Edit `config.json` before launching:
   - Replace `REPLACE_ME_WITH_PATH/plex_epg.db` with the path to your Plex EPG database.
   - Replace `REPLACE_ME_WITH_PATH/Movies` with one or more directories that contain your owned films.
   - Replace `REPLACE_ME` in `omdb_api_key` with your personal OMDb key (leave empty to use the demo key, but expect heavy rate limiting).
4. Double-click `pex.exe` (or run `./pex` on Linux/macOS).

Notes
-----
- The first run scans the Plex database and your owned library; it can take a while on large collections.
- Subsequent runs reuse cached data stored in `.pex_cache/`.
- The app expects `ffprobe` to be in `PATH` (or set `ffprobe_cmd` in `config.json`).