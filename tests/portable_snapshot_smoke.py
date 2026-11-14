"""Smoke-test a portable PEX ZIP on Windows or Linux (use Xvfb on headless Linux)."""

from contextlib import closing, contextmanager
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
from zipfile import ZipFile


EXECUTABLE = "pex.exe" if os.name == "nt" else "pex"
SCHEMA = (Path(__file__).resolve().parents[1] / "src/app/test_fixtures.sql").read_text()


def seed(database, title):
    with closing(sqlite3.connect(database)) as db:
        db.executescript(SCHEMA)
        db.execute("UPDATE metadata_items SET title=?, original_title=?, user_thumb_url=?",
                   (title, title, "http://127.0.0.1:9/fixture"))
        db.execute("UPDATE media_items SET begins_at=?", (int(time.time()) + 600,))
        db.commit()


def assert_title(database, expected):
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as db:
        assert db.execute("SELECT title FROM metadata_items WHERE id=1").fetchone() == (expected,)


def launch(app, expected_title, live_copy=False):
    root = app.parent
    mirror = app / "db/plex_library.db"
    cache = app / ".pex_cache/owned_titles_v5.json"
    config = json.loads((app / "pex-config.json").read_text(encoding="utf-8"))
    cache.unlink(missing_ok=True)  # A successful scan must regenerate this file.
    env = dict(os.environ, PEX_CONFIG=str(app / "pex-config.json"), PEX_DISABLE_PREFETCH="1",
               HOME=str(root), USERPROFILE=str(root), APPDATA=str(root / "roaming"),
               LOCALAPPDATA=str(root / "local"), XDG_CONFIG_HOME=str(root / "config"),
               XDG_CACHE_HOME=str(root / "cache"))
    env.pop("PEX_BASE_DIR", None)
    startup = None
    if os.name == "nt":
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0
    with tempfile.TemporaryFile() as log, subprocess.Popen(
        [str(app / EXECUTABLE)], cwd=root, env=env, startupinfo=startup,
        stdout=log, stderr=subprocess.STDOUT,
    ) as proc:
        try:
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                if proc.poll() is not None:
                    log.seek(0)
                    details = log.read().decode(errors="replace")
                    raise RuntimeError(f"Fixture PEX exited early ({proc.returncode}):\n{details[-4000:]}")
                if (app / "db/plex_epg.db.last_sync").is_file() and cache.is_file():
                    saved = json.loads(cache.read_text(encoding="utf-8"))
                    if "library_snapshot_dir" in config:
                        assert saved["source"] == {"Snapshots": config["library_snapshot_dir"]}
                        assert saved["titles"] == {}
                        break
                    if saved["titles"].get(expected_title.lower() + ":2020") == "hd":
                        break
                time.sleep(0.2)
            else:
                raise RuntimeError(f"Timed out waiting for guide and owned entry {expected_title}")
            assert_title(app / "db/plex_epg.db", "Snapshot Fixture")
            assert_title(mirror, expected_title)
            assert (app / "db/plex_library.db.last_sync").exists() == live_copy
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=5)


def source_state(folder):
    return {
        file.relative_to(folder): (file.stat().st_mtime_ns, hashlib.sha256(file.read_bytes()).digest())
        for file in folder.rglob("*") if file.is_file()
    }


@contextmanager
def readonly_source(folder):
    modes = {file: file.stat().st_mode for file in folder.rglob("*") if file.is_file()}
    try:
        for file, mode in modes.items():
            file.chmod(mode & ~(stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))
        before = source_state(folder)
        yield
        assert source_state(folder) == before, f"PEX changed files in source directory {folder}"
    finally:
        for file, mode in modes.items():
            file.chmod(mode)


def smoke(bundle, root):
    app = root / "portable"
    app.mkdir()
    with ZipFile(bundle) as archive:
        # Only extract known flat package members to paths we control.
        assert "pex_snapshot.py" not in archive.namelist()
        for name in [EXECUTABLE, "config.json"]:
            with archive.open(name) as src, (app / name).open("xb") as dst:
                shutil.copyfileobj(src, dst)
    (app / EXECUTABLE).chmod(0o755)
    if os.name == "nt" and os.environ.get("SNAPSHOT_TEST_SWIFTSHADER"):
        # SwiftShader supports direct loading without a system Vulkan installation.
        shutil.copyfile(os.environ["SNAPSHOT_TEST_SWIFTSHADER"], app / "vulkan-1.dll")
    data = root / "Plex Databases"
    data.mkdir()
    backups = root / "Completed Plex backups"
    backups.mkdir()
    guide = data / "tv.plex.providers.epg.cloud-smoke.db"
    library = data / "com.plexapp.plugins.library.db"
    seed(guide, "Snapshot Fixture")
    seed(library, "Live Library")
    config = {
        "plex_epg_db_source": str(guide),
        "plex_library_db_source": str(library),
    }
    config_file = app / "pex-config.json"
    config_file.write_text(json.dumps(config))
    with readonly_source(data):
        launch(app, "Live Library", live_copy=True)

    # Make the mirror due for a refresh. Keep a committed title change in the
    # fixture's WAL while the packaged app copies it over the old local mirror.
    marker = app / "db/plex_library.db.last_sync"
    old_time = time.time() - 2 * 24 * 60 * 60
    os.utime(marker, (old_time, old_time))
    with closing(sqlite3.connect(library)) as writer:
        assert writer.execute("PRAGMA journal_mode=WAL").fetchone() == ("wal",)
        writer.execute("PRAGMA wal_autocheckpoint=0")
        writer.execute("UPDATE metadata_items SET title=?, original_title=?",
                       ("Updated Live Library", "Updated Live Library"))
        writer.commit()
        assert (data / (library.name + "-wal")).stat().st_size > 0
        with readonly_source(data):
            launch(app, "Updated Live Library", live_copy=True)
    assert marker.stat().st_mtime > old_time

    # A busy first scan uses an existing dated backup. Each directory below
    # represents the state of a server share before PEX opens the app.
    fallback_app = root / "fallback-portable"
    fallback_app.mkdir()
    shutil.copy2(app / EXECUTABLE, fallback_app / EXECUTABLE)
    if (app / "vulkan-1.dll").is_file():
        shutil.copy2(app / "vulkan-1.dll", fallback_app / "vulkan-1.dll")
    (fallback_app / "pex-config.json").write_text(json.dumps(config))
    seed(data / (library.name + "-2024-01-01"), "Fallback Backup")
    (data / (library.name + "-journal")).write_bytes(b"busy fixture")
    with readonly_source(data):
        launch(fallback_app, "Fallback Backup")

    # Snapshot-only mode works with read access to already completed files.
    seed(backups / (library.name + "-2024-01-02"), "Saved Backup")
    config["library_snapshot_dir"] = str(backups)
    config["plex_library_db_source"] = str(data / "absent-live-source.db")
    config_file.write_text(json.dumps(config))
    mirror = app / "db/plex_library.db"
    with readonly_source(data), readonly_source(backups):
        launch(app, "Saved Backup")
        stamp = mirror.stat().st_mtime_ns
        digest = hashlib.sha256(mirror.read_bytes()).digest()
        launch(app, "Saved Backup")
        assert mirror.stat().st_mtime_ns == stamp
        assert hashlib.sha256(mirror.read_bytes()).digest() == digest
    seed(backups / (library.name + "-2024-01-03"), "Newer Backup")
    with readonly_source(data), readonly_source(backups):
        launch(app, "Newer Backup")
        assert hashlib.sha256(mirror.read_bytes()).digest() != digest
    print("Portable smoke passed for live, busy and pre-existing backup sources.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python tests/portable_snapshot_smoke.py <portable ZIP>")
    with tempfile.TemporaryDirectory(prefix="pex-snapshot-smoke-") as temp:
        smoke(Path(sys.argv[1]).resolve(), Path(temp).resolve())
