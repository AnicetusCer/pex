# PEX – Plex EPG Explorer (portable)

## First launch on Windows

1. Extract the ZIP into a new folder where you have write access. Extract it fully before launching `pex.exe`. SQLite is included; you do not need to install SQLite or copy a config from another PC.
2. Double-click `pex.exe`. **Connect to Plex** opens when the guide has not been configured.
3. Choose **Browse...** and select your Plex server's `Plug-in Support/Databases` folder. For a NAS, you can paste a Windows network path such as `\\NAS\PlexMediaServer\AppData\Plex Media Server\Plug-in Support\Databases` and click **Find databases**. Connect/authenticate the share in File Explorer first if Windows requires credentials. Enter paths normally; no JSON escaping is needed in the app.
4. PEX lists `tv.plex.providers.epg.cloud*.db` guide databases. If several are listed, choose the one for your DVR guide. PEX also finds `com.plexapp.plugins.library.db` in the same folder.
5. When PEX finds a library, owned/HD and scheduled recording badges are enabled. Check **Guide only** if you just want listings. Click **Save and load guide**.

Plex DVR/EPG must be enabled on your server. No Plex token or TMDb key is needed. Poster downloads need internet access; listings and local caches remain usable without it.

Settings are saved automatically. PEX creates `db/` and `.pex_cache/` beside the executable. Keep this portable folder writable. Use **Connect to Plex** in the guide to change the connection later; saving is available after active scans and downloads finish.

## Refresh from completed snapshots

In **Connect to Plex**, choose your guide, clear **Guide only**, and select **Completed snapshots**. Use Plex's Databases folder if it holds the backups, or select the backup folder as it appears on the **computer running PEX**. This can be a local directory, a Windows UNC path, or a mounted share. Save the connection.

The simplest source is Plex's **Settings → Server → Scheduled Tasks** option to back up the database every three days. PEX recognizes completed files named `com.plexapp.plugins.library.db-YYYY-MM-DD`. Plex also supports a custom backup location. The first backup may take several days to appear. [Plex scheduled tasks](https://support.plex.tv/articles/201553286-scheduled-tasks/)

Use **Refresh owned scan** to check the folder. PEX copies and validates a new snapshot locally. If the same snapshot is already installed and usable, PEX reads it locally and skips the full database transfer. The date shown in PEX tells you how current the owned and recording badges are. The programme guide keeps its own source. Select **Live library** to return to live copies.

### Create a fresher snapshot while Plex runs

If you administer the Plex server, the ZIP includes an optional `pex_snapshot.py` exporter. It needs Python 3.8 or later with the standard `sqlite3` module **on the Plex host**. Run it when you need a fresh copy, or schedule the same command with your server's task scheduler. Each run takes a consistent SQLite snapshot and publishes one timestamped database. [SQLite online backup](https://sqlite.org/backup.html)

Pass the full **server-local** library file and a dedicated output folder on local storage. From the directory containing the script, run the command for your host.

```powershell
# Windows Plex server (PowerShell)
py -3 .\pex_snapshot.py "$env:LOCALAPPDATA\Plex Media Server\Plug-in Support\Databases\com.plexapp.plugins.library.db" "D:\PEX snapshots"
```

```sh
# Linux, macOS or NAS; replace the two paths for your installation
python3 pex_snapshot.py "/path/to/Databases/com.plexapp.plugins.library.db" "/path/to/PEX snapshots"
```

For Docker, run the command on the server with access to Plex's actual data volume. The exporter account needs access to the database and SQLite sidecars and permission to create files in the output directory. Make the completed exports readable to the PEX client, then select that folder under **Completed snapshots**. On appliances where you cannot run Python, use Plex's scheduled backups. [Plex data locations by platform](https://support.plex.tv/articles/202915258-where-is-the-plex-media-server-data-directory-located/)

Each export uses roughly one library database's worth of disk space. When scheduling exports, set up retention for this dedicated folder and allow space for the next export. The helper leaves older snapshots in place. On Linux, macOS and NAS systems, choose an output filesystem that supports hard links so a completed export can appear atomically. Keep the server and client clocks in sync.

## When the live library is busy

The guide can load while the owned library is copied and scanned in the background. PEX makes up to three attempts to copy the live library, checking that its database and WAL have not changed during the copy. It rebuilds SQLite state and validates the copy locally; it never opens SQLite against your server's files.

If the live copy fails:

- A usable previous local mirror is retained.
- If there is no usable local mirror, PEX looks for completed Plex backups alongside the configured live library, named exactly `com.plexapp.plugins.library.db-YYYY-MM-DD`. Newest dates are tried first. Files with active WAL/journal data, invalid dates, invalid SQLite contents, or an incompatible library schema are rejected.
- When a backup is used, the guide shows its filename and date. Owned and recording badges reflect that snapshot, so recent library additions or scheduling changes may be absent. Your **live library source remains configured** for future refreshes.
- If no valid snapshot is available, an explanation stays visible above the guide with **Retry owned scan**. Check the share or wait for Plex to finish its activity/backup, then retry. You can also use **Connect to Plex → Guide only** and browse without owned or recording badges.

A backup does not count as a successful daily live refresh. PEX tries the live source again on the next launch; **Retry owned scan** forces a live refresh immediately. **Advanced → Refresh owned scan** remains available.

## Existing portable installations

Close PEX and extract the new ZIP over your existing portable folder. Your saved `pex-config.json`, local databases and caches stay in place. If you are upgrading an older install with settings only in `config.json`, copy that file to `pex-config.json` before extracting. The ZIP replaces the starter `config.json`.

PEX searches for `PEX_CONFIG`, `pex-config.json` beside the executable, that filename in the current directory, `~/.pex-config.json`, then legacy `config.json` beside the executable/current directory. Local manually supplied `db/plex_epg.db` and `db/plex_library.db` remain supported. Existing configs do not require the new `guide_only` key.

When you save a connection, PEX preserves other keys, including your cache directory and TMDb key. It saves to `pex-config.json` when starting from the bundled or a legacy `config.json`, leaving that legacy file intact. This protects the saved connection when a later ZIP replaces its bundled starter config. Existing preferred/home configs are updated in place, and an explicit `PEX_CONFIG` destination is always honored. Malformed JSON is reported and must be repaired before saving; it is not overwritten. The bundled empty `config.json` starts the setup screen. Extract a preview or update into a separate folder to try it without changing your working installation.

## Optional configuration

| Key | Purpose |
| --- | --- |
| `plex_epg_db_source` | Live guide database path, filled in by setup. |
| `plex_library_db_source` | Live library database path, filled in by setup; optional. |
| `library_snapshot_dir` | Folder of completed library snapshots as seen by PEX. When set, owned refreshes use this folder. |
| `guide_only` | `true` skips the owned scan and recording badges, including existing library mirrors. Default `false`. |
| `cache_dir` | Cache location; relative paths resolve beside the executable. Default `.pex_cache`. |
| `tmdb_api_key` | Optional personal TMDb V3 key for on-demand ratings. |
| `log_level` | Logging level, default `info`. |

For manual JSON edits, escape Windows backslashes or use forward slashes. `config_example.windows` and `config_example.linux` contain examples. The app accepts UNC paths directly in the folder field.

## Troubleshooting

- **Folder cannot be read:** open the same share in File Explorer as the same Windows user, check permissions, and use **Find databases** again.
- **No guide databases found:** select the `Databases` folder itself and ensure Plex DVR/EPG has been configured.
- **Guide cannot load:** PEX returns to **Connect to Plex**, showing the error and a next action. Choose another guide when several are listed, or correct the saved file and use **Retry saved configuration**.
- **No owned badges:** check the persistent snapshot/error message above the guide, then **Retry owned scan** or reconnect. A backup may predate recent additions.
- **No films:** try a wider day range and clear filters. Your Plex guide may need refreshing on the server.
- **No posters:** check internet access. Use **Advanced → Clear & rebuild poster cache** after downloads finish.

The portable bundle contains the executable, empty starter config, README, sample configs, optional snapshot exporter, LICENSE and NOTICE.

[Full project documentation](https://github.com/AnicetusCer/pex#readme)
