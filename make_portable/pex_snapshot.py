#!/usr/bin/env python3
"""Publish a standalone PEX library snapshot from the machine running Plex.

Requires Python 3.8+ with sqlite3. Use the server's local filesystem path to
the live database. Share the export directory with PEX. Each successful run
writes a new full snapshot; the server administrator controls retention.
"""

import argparse
import errno
import os
import sqlite3
import sys
import uuid
from contextlib import closing
from datetime import datetime, timezone
from pathlib import Path

BUSY_TIMEOUT_SECONDS = 5


def generation_name():
    return datetime.now(timezone.utc).strftime("pex-library-%Y-%m-%dT%H%M%S%fZ.db")


def is_windows_share(path):
    import ctypes

    # Resolving drive letters usually expands a mapped drive to a UNC path.
    # GetDriveTypeW also covers mappings that pathlib did not expand.
    return path.drive.startswith("\\\\") or ctypes.windll.kernel32.GetDriveTypeW(path.anchor) == 4


def export_snapshot(library, export_dir):
    source = library.resolve(strict=True)
    if not source.is_file():
        raise ValueError("The library path must be the Plex library database file.")
    if os.name == "nt" and is_windows_share(source):
        raise ValueError("Run the exporter on the Plex server with a local database path.")

    export_dir.mkdir(parents=True, exist_ok=True)
    staging = export_dir / (".pex-export-" + uuid.uuid4().hex + ".tmp")
    # Exclusive creation also lets the directory ACL and normal umask govern
    # access to the published file. A reader never needs write access.
    with staging.open("xb"):
        pass
    try:
        uri = source.as_uri() + "?mode=ro"
        with closing(sqlite3.connect(uri, uri=True, timeout=BUSY_TIMEOUT_SECONDS)) as src:
            src.execute("BEGIN")
            # A real read establishes the snapshot. Stepped online backup then
            # uses the same read transaction even if Plex commits between steps.
            tables = src.execute(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND "
                "name IN ('metadata_items', 'media_items', 'media_parts')"
            ).fetchone()[0]
            if tables != 3:
                raise ValueError("The file is missing required Plex library tables.")
            final_path = export_dir / generation_name()
            busy_steps = 0

            def progress(status, remaining, total):
                nonlocal busy_steps
                if status in (5, 6):  # SQLITE_BUSY and SQLITE_LOCKED
                    busy_steps += 1
                    if busy_steps >= 20:
                        raise RuntimeError("SQLite is busy. Try the export again later.")

            with closing(sqlite3.connect(str(staging), timeout=BUSY_TIMEOUT_SECONDS)) as dst:
                src.backup(dst, pages=256, progress=progress, sleep=0.25)
                src.rollback()  # Release the pinned reader as soon as copying finishes.
                if dst.execute("PRAGMA journal_mode=DELETE").fetchone()[0] != "delete":
                    raise RuntimeError("Could not finish a standalone SQLite snapshot.")
        if os.name == "nt":
            os.rename(str(staging), str(final_path))  # Windows refuses to overwrite.
        else:
            # Atomic and no-clobber even if two jobs produce the same timestamp.
            try:
                os.link(str(staging), str(final_path))
            except OSError as exc:
                if exc.errno in (errno.EPERM, errno.ENOTSUP, errno.ENOSYS):
                    raise RuntimeError(
                        "Choose a local export folder that allows hard links."
                    ) from exc
                raise
        return final_path
    finally:
        for suffix in ("-wal", "-shm", "-journal", ""):
            Path(str(staging) + suffix).unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path, help="local com.plexapp.plugins.library.db")
    parser.add_argument("export_directory", type=Path, help="directory to share with PEX")
    args = parser.parse_args()
    try:
        result = export_snapshot(args.library, args.export_directory)
    except (OSError, ValueError, sqlite3.Error, RuntimeError) as exc:
        print("Snapshot export failed: {}".format(exc), file=sys.stderr)
        return 1
    print(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
