# Unreleased

- A manual refresh makes one read-only copy attempt against the live Plex library. If Plex changes it during the copy, PEX can use its previous mirror or an existing completed library backup.
- **Completed snapshots** lets PEX read backups from an existing folder. The guide shows the backup date for owned and scheduled recording badges. All scanning and SQLite validation take place on the PEX computer.

# Pex 1.2.5

- On first launch, **Connect to Plex** lets you select your Plex Databases folder or paste a Windows network share. PEX finds the guide and library databases and lets you choose the guide if several are available. Working portable configurations continue to load.
- Select **Guide only** to skip the owned-library scan. You can enable owned-library and recording badges later from **Connect to Plex**.
- To upgrade, close PEX and extract the ZIP into your existing folder. Your saved `pex-config.json` stays in place. If an older install has settings only in `config.json`, copy it to `pex-config.json` before extracting.
- If Plex is busy, PEX tries the live library up to three times. It keeps a usable local snapshot. On a first scan it can use the newest completed, valid dated backup in the Databases folder. The guide shows which snapshot supplied the badges and when it was taken or copied.
- The live library stays configured. Use **Retry owned scan** to refresh it from the guide. If the network share is unavailable or no usable snapshot exists, PEX shows the next action in the app.
