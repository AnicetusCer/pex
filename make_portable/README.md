# Pex (Plex EPG Explorer)

![Film discovery grid](../src/assets/PEXFilmEPG.png)

If you're old-fashioned like me and still enjoy flicking through the next week of TV films, this app is for you. Pex is my personal way to dig through two weeks of Plex DVR listings and decide what to record, and—most importantly—see which airings I already own in Plex, all in a super-visual grid.

Pex helps you browse upcoming film broacasts in a more advanced way:
- Browse up to 14 days of film listings in the Plex EPG with a poster-forward layout.
- Choose to visually dim movies you already own or hide them from the grid entirely.
- Spot HD airings, including HD upgrades for titles you currently only have in SD.
- See at a glance which films are already scheduled to record in Plex (does not support schedualling, i want to keep the app focused on browsing)
- Bring channel art, genre groupings, and on-demand TMDb ratings (click the ⭐ button in the detail pane) into the experience while keeping everything cached locally for speedy, offline-friendly launches.

If you downloaded this release bundle from the main repo, all you need to do is fill in `config.json`. Point the entries at your Plex EPG and library databases and you’re done—Pex mirrors Plex’s own data to decide what you already own, so there’s no filesystem crawl to configure.

Because the app is written in Rust, the portable build runs on Windows, Linux, and macOS. It’s easy to tweak too—grab the source, point your favourite AI at the primer file I included, and you’ll have a head start on customising the code for your setup. The most likely change people will make is adapting the title parsing if their files don’t follow the `Title (Year)` pattern the app expects today.

Need a head start on the paths? See `config_example.windows` and `config_example.linux` in this folder for realistic sample values covering common Plex installations and multi-library layouts.

---

## What’s inside?

- `pex.exe` (Windows) or `pex` (Linux/macOS) – the application binary.
- `config.json` – edit this to match your Plex setup and movie library.
- `README.md` – this guide.
- `db/` (created on first run) – Pex will copy your Plex EPG database here.
- `.pex_cache/` (created on first run) – stores posters, owned sidecars, and UI preferences.

---

## Quick start

1. **Unzip / extract the bundle** to a folder where you have write access. Pex
   stores cache files alongside the executable.

2. **Install prerequisites**
   - Ensure Plex DVR/EPG is enabled so its SQLite database exists.
   - No extra SQLite packages are required on Windows: the portable binary ships
     with SQLite via `rusqlite`'s bundled driver. On Linux the portable build
     also links SQLite statically; just make sure you're on a glibc-based
     distro.

3. **Edit `config.json`** – update the Plex database paths and optional settings so they match the machine you will run Pex on. Each key is described in detail in [Configuration keys](#configuration-keys) below.

4. **Launch the app**
   - Windows: double-click `pex.exe` or run it from PowerShell.
   - Linux/macOS: `chmod +x ./pex` (if needed) then run `./pex`.

Pex will make local copies of your Plex databases to avoid any chance of disruption. The first start can take a few minutes while posters download and owned sidecars are generated, but subsequent launches reuse the cache and start quickly.
General workflow:
- Pex copies the Plex database from `plex_epg_db_source`.
- It copies Plex’s library database if `plex_library_db_source` is set.
- Owned titles are tagged straight from the mirrored library database.
- It warms up posters into the grid.

Subsequent launches load almost immediately.

---

## Configuration keys

| Key | Required? | Description | Where to find the value |
| --- | --- | --- | --- |
| `plex_epg_db_source` | ✅ | Absolute path to Plex's EPG database (`tv.plex.providers.epg.cloud*.db`). Pex copies it into `db/plex_epg.db` the first time you launch and refreshes the copy roughly once per day. | See [Collecting Plex paths](#collecting-plex-paths) for examples. |
| `plex_library_db_source` | ✅ | Path to `com.plexapp.plugins.library.db`. Mirroring this database enables owned detection and the DVR *REC* badge. | See [Collecting Plex paths](#collecting-plex-paths) for examples. |
| `cache_dir` | Optional | Relocates `.pex_cache/` (posters, owned sidecars, UI prefs). Relative paths are resolved next to the executable. | Pick a writable folder with enough free space. |
| `tmdb_api_key` | Optional | TMDb V3 API key for vote-average ratings. | Generate your key at <https://www.themoviedb.org/settings/api>. |
| `log_level` | Optional | Adjusts runtime logging (`trace`, `debug`, `info`, `warn`, `error`). | Set only if you need more verbose console output. |

### Collecting Plex paths

Plex stores its SQLite databases under “Plug-in Support/Databases”. Common locations:

| Platform | Default path |
| --- | --- |
| **Windows (desktop)** | `%LOCALAPPDATA%\Plex Media Server\Plug-in Support\Databases\` |
| **Windows (service install)** | `%PROGRAMDATA%\Plex Media Server\Plug-in Support\Databases\` |
| **Linux packages** | `/var/lib/plexmediaserver/Library/Application Support/Plex Media Server/Plug-in Support/Databases/` |
| **Synology package** | `/var/packages/Plex Media Server/target/Plex Media Server/Plug-in Support/Databases/` |
| **Docker** | Host folder bound to `/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/` |

Look in that directory for:

- `tv.plex.providers.epg.cloud.db`
- `com.plexapp.plugins.library.db`

Copy the full paths into `config.json`. Pex pulls in any companion journal data
for you, so you only need the main `.db` path.

---

## Where files are stored

| Location | Purpose |
| --- | --- |
| `db/plex_epg.db` | Working copy of Plex's EPG database. |
| `db/plex_library.db` | Working copy of Plex's library database (refreshed when configured). |
| `.pex_cache/` | Posters, owned sidecars, and UI preferences. Poster images older than 14 days are removed automatically. |
| `config.json` | Runtime configuration – edit before launching. |

You can delete `.pex_cache/` if you want to reclaim space; Pex rebuilds it on
the next run (expect another long initial scan while it repopulates).

---

## Windows vs. Linux/macOS differences

- **Paths** – stick to forward slashes in `config.json`. Windows backslashes
  must be escaped (`"D:\\\\Media"`), so forward slashes are easier.
- **Executable name** – Windows builds end with `.exe`; Linux/macOS builds are
  just `pex`. The rest of the bundle layout is identical.

---

## Troubleshooting

- **“Could not open Plex DB”** – check `plex_epg_db_source` or ensure
  `db/plex_epg.db` exists and is readable.
- **Owned scan never finishes** – ensure `plex_library_db_source` points at a readable Plex library database and that the mirrored `db/plex_library.db` is up to date.
- **No posters or very slow start** – verify the machine has internet access and
  that `.pex_cache/` is writable.
- **HD badge seems wrong** – run the app, open **Advanced ▸ Refresh owned scan**, and let Pex rebuild the owned sidecars from the Plex library database.

If you need a deeper reference, open the repository’s main guide:
<https://github.com/AnicetusCer/pex#readme>.

Enjoy Pex!
