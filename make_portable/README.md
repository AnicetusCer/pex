# Welcome to Pex Portable

This folder contains a ready-to-run copy of **Pex – Plex EPG Explorer**. If you
received this bundle, you do **not** need the Rust toolchain; follow the steps
below to get going.

---

## What’s inside?

- `pex.exe` (Windows) or `pex` (Linux/macOS) – the application binary.
- `config.json` – edit this to match your Plex setup and movie library.
- `README.md` – this guide.
- `db/` (created on first run) – Pex will copy your Plex EPG database here.
- `.pex_cache/` (created on first run) – stores posters, owned manifest, and ffprobe cache.

---

## Quick start

1. **Unzip / extract the bundle** to a folder where you have write access. Pex
   stores cache files alongside the executable.

2. **Install prerequisites**
   - Ensure Plex DVR/EPG is enabled so its SQLite database exists.
   - Install FFmpeg (or just `ffprobe`) and make sure the `ffprobe` command is
     available. If it isn’t, you can point Pex to the executable in the config.

3. **Edit `config.json`**
   - `library_roots`: list the folders that contain your own movie files. These
     paths must be valid on this machine. Use forward slashes on every OS, e.g.:
     - Windows: `"D:/Media/Movies"`
     - Linux: `"/mnt/media/movies"`
     Name your files in the usual Plex style: `Title (Year).ext`. Any trailing
     suffixes (such as `- 4K` or `- Directors Cut`) are ignored automatically
     during scanning and parsing so they still match the Plex guide entries.
   - `plex_db_source`: path to Plex’s live DVR database (for example
     `"C:/Users/You/AppData/Local/Plex Media Server/Plug-in Support/Databases/tv.plex.providers.epg.cloud.db"`
     on Windows). Pex copies this file into `db/plex_epg.db` the first time you
     launch and refreshes the copy roughly once per day. This value is required.
   - `ffprobe_cmd`: override the location of ffprobe (e.g.
     `"C:/Tools/ffmpeg/bin/ffprobe.exe"` or `"/usr/bin/ffprobe"`). Leave it
     blank to use `ffprobe` from the system `PATH`.
   - `omdb_api_key`: OMDb API key used for IMDb ratings. The bundled config
     ships with the public demo key (`thewdb`), which is heavily rate-limited.
     Replace it with your personal key if you plan to use rating lookups (click
     the ratings button in a movie selection to trigger a lookup).

4. **Launch the app**
   - Windows: double-click `pex.exe` or run it from PowerShell.
   - Linux/macOS: `chmod +x ./pex` (if needed) then run `./pex`.

The first start can take a long time (30 minutes or more for ~6,000 movies on a nas):
- Pex copies the Plex database from `plex_db_source`.
- It scans your `library_roots` to tag owned titles.
- It warms up poster and ffprobe caches.

Subsequent launches load almost immediately.

---

## Where files are stored

| Location | Purpose |
| --- | --- |
| `db/plex_epg.db` | Working copy of Plex’s EPG database (plus WAL/SHM files). |
| `.pex_cache/` | Posters, owned-manifest, ffprobe cache, and UI preferences. Poster images older than 14 days are removed automatically. |
| `config.json` | Runtime configuration – edit before launching. |

You can delete `.pex_cache/` if you want to reclaim space; Pex rebuilds it on
the next run (expect another long initial scan while it repopulates).

---

## Windows vs. Linux/macOS differences

- **Paths** – stick to forward slashes in `config.json`. Windows backslashes
  must be escaped (`"D:\\\\Media"`), so forward slashes are easier.
- **ffprobe location** – on Windows, FFmpeg installers typically place
  `ffprobe.exe` under `C:/Program Files/FFmpeg/bin`. On Linux it’s usually in
  `/usr/bin/ffprobe`.
- **Executable name** – Windows builds end with `.exe`; Linux/macOS builds are
  just `pex`. The rest of the bundle layout is identical.

---

## Troubleshooting

- **“Could not open Plex DB”** – check `plex_db_source` or ensure
  `db/plex_epg.db` exists and is readable.
- **Owned scan never finishes** – confirm each `library_root` path exists and
  that the drive/share is mounted.
- **No posters or very slow start** – verify the machine has internet access and
  that `.pex_cache/` is writable.
- **HD badge seems wrong** – run the app, open **Advanced** ▸ **Refresh ffprobe
  cache**, and make sure `ffprobe_cmd` points to a modern FFmpeg build.

If you need a deeper reference, open the repository’s main guide:
<https://github.com/AnicetusCer/pex#readme>.

Enjoy Pex!
