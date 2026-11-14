# Unreleased

- The guide keeps repeat airings, remakes and films without artwork. Each card reflects the current filters. Expand **Other showings (N)** for additional times and channels. **REC** identifies the displayed showing and **Other REC** identifies a scheduled alternative.
- Film listings appear while posters load in the background. Showings with the same artwork share a poster download and display texture. Selecting a film loads its poster in Details even when its card is offscreen.
- Recording badges distinguish channels airing the same film at the same time. **REC?** identifies a scheduled time with an unconfirmed channel. **Film REC** identifies a recording request without a confirmed showing.
- PEX now recognises Plex recording subscriptions that store the station ID and display name together. A unique station match at the booked time gets **REC**, including when one subscription lists several channels and airtimes.
- Recording badges recognise zero-padded channel numbers, and an unreadable recording entry no longer hides other bookings.
- The guide labels confirmed HD and SD showings and leaves the quality label off when the guide supplies insufficient evidence. PEX checks whether Plex analysed the media before using its dimensions, preventing false **HD ↑** badges on listings such as Freeview Sky Mix.
- Owned badges follow the configured library across restarts. **Owned Details** includes both neighbouring years when they tie for the best match.
- **HD ↑** requires an HD airing and confirmed SD library copies. Files with missing dimensions show **Owned**. Manual and completed-snapshot scans skip saving redundant title lists.
- **Owned Details** explains the match and shows the Plex title, year and server file path. It lists all candidates when a match is ambiguous. The scan total counts each Plex movie once, including movies with multiple files or title variants.
- **Advanced → Refresh path status** checks configured sources and local paths in the background, so an unreachable share leaves the guide available.
- Manage an optional TMDb v3 API key from **Set up ratings…** on **Connect to Plex** or **Advanced → Ratings key…**. Choosing **⭐ Rating** in film details opens the same masked key entry when needed and resumes the lookup after saving. The key is saved with your portable settings.
- Invalid config messages identify the file and JSON error location without quoting private settings.
- A live owned refresh checks the library and its WAL twice before transferring either file. PEX skips the live copy if the files are changing and bounds each transfer by its starting size. When the live source is busy, PEX keeps its previous usable mirror or tries an existing completed library backup.
- **Completed snapshots** lets PEX read backups from an existing folder. The guide shows the backup date for owned and scheduled recording badges. All scanning and SQLite validation take place on the PEX computer.

# Pex 1.2.5

- On first launch, **Connect to Plex** lets you select your Plex Databases folder or paste a Windows network share. PEX finds the guide and library databases and lets you choose the guide if several are available. Working portable configurations continue to load.
- Select **Guide only** to skip the owned-library scan. You can enable owned-library and recording badges later from **Connect to Plex**.
- To upgrade, close PEX and extract the ZIP into your existing folder. Your saved `pex-config.json` stays in place. If an older install has settings only in `config.json`, copy it to `pex-config.json` before extracting.
- If Plex is busy, PEX tries the live library up to three times. It keeps a usable local snapshot. On a first scan it can use the newest completed, valid dated backup in the Databases folder. The guide shows which snapshot supplied the badges and when it was taken or copied.
- The live library stays configured. Use **Retry owned scan** to refresh it from the guide. If the network share is unavailable or no usable snapshot exists, PEX shows the next action in the app.
