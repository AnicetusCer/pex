# PEX – Plex EPG Explorer (portable)

## First launch on Windows

1. Extract the ZIP into a new folder where you have write access. Extract it fully before launching `pex.exe`. SQLite is included; you do not need to install SQLite or copy a config from another PC.
2. Double-click `pex.exe`. **Connect to Plex** opens when the guide has not been configured.
3. Choose **Browse...** and select your Plex server's `Plug-in Support/Databases` folder. For a NAS, you can paste a Windows network path such as `\\NAS\PlexMediaServer\AppData\Plex Media Server\Plug-in Support\Databases` and click **Find databases**. Connect/authenticate the share in File Explorer first if Windows requires credentials. Enter paths normally; no JSON escaping is needed in the app.
4. PEX lists `tv.plex.providers.epg.cloud*.db` guide databases. If several are listed, choose the one for your DVR guide. PEX also finds `com.plexapp.plugins.library.db` in the same folder.
5. When PEX finds a library, owned/HD and scheduled recording badges are enabled. Check **Guide only** if you just want listings. Click **Save and load guide**.

Plex DVR/EPG must be enabled on your server. No Plex token or TMDb key is needed. Poster downloads need internet access; listings and local caches remain usable without it.

Settings are saved automatically. PEX creates `db/` and `.pex_cache/` beside the executable. Keep this portable folder writable. Use **Connect to Plex** in the guide to change the connection later; saving is available after active scans and downloads finish.

## Refresh the owned library

In **Live library** mode, **Refresh owned scan** checks the database and its SQLite write-ahead log (WAL) twice before starting a transfer. If the files are changing, PEX skips the live copy. If they look stable, PEX copies them once, checks again for changes and validates the copy on your computer. You can keep using Plex and browsing the guide during the refresh.

If Plex starts writing during the transfer, PEX limits the read to the file sizes it saw at the start and discards a copy that fails its consistency checks. When the live copy is unavailable, PEX keeps a usable previous mirror from the configured source. If there is no usable mirror, PEX looks for an existing, completed `com.plexapp.plugins.library.db-YYYY-MM-DD` backup alongside the live library. It tries the newest dates first and checks that each candidate is complete, stable and readable as a Plex library.

The guide shows the source and date used for owned and scheduled recording badges. A backup can predate new films, recording requests or cancellations. Choose **Retry owned scan** to check the live library again. If the share is unavailable, you can keep browsing using the saved mirror or select **Guide only**.

## Refresh from completed snapshots

If you already have completed Plex backups, you can use them directly. In **Connect to Plex**, select your guide, clear **Guide only**, choose **Completed snapshots**, and select the folder containing the backups. Enter the path as seen by the **computer running PEX**. It can be a local folder, a Windows UNC path, or a mounted share.

**Refresh owned scan** checks that folder for a newer backup. PEX reuses the validated local mirror when the same backup is already installed. The guide shows the backup date. Use **Live library** to return to live copies. Both modes work with read-only access to the Plex files and store working data on the PEX computer.

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

The portable bundle contains the executable, empty starter config, README, sample configs, LICENSE and NOTICE.

[Full project documentation](https://github.com/AnicetusCer/pex#readme)
