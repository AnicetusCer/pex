"""Smoke-test a portable PEX ZIP on Windows or Linux (use Xvfb on headless Linux)."""

from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
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


def launch(app, expected_title):
    root = app.parent
    mirror = app / "db/plex_library.db"
    cache = app / ".pex_cache/owned_all_v3.txt"
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
                    if expected_title.lower() + ":2020" in cache.read_text().splitlines():
                        break
                time.sleep(0.2)
            else:
                raise RuntimeError(f"Timed out waiting for guide and owned entry {expected_title}")
            assert_title(app / "db/plex_epg.db", "Snapshot Fixture")
            assert_title(mirror, expected_title)
            assert not (app / "db/plex_library.db.last_sync").exists()
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=5)


def smoke(bundle, root):
    app = root / "portable"
    app.mkdir()
    with ZipFile(bundle) as archive:
        # Only extract known flat package members to paths we control.
        for name in [EXECUTABLE, "pex_snapshot.py", "config.json"]:
            with archive.open(name) as src, (app / name).open("xb") as dst:
                shutil.copyfileobj(src, dst)
    (app / EXECUTABLE).chmod(0o755)
    data = root / "Plex Databases"
    data.mkdir()
    exports = root / "Shared snapshots"
    exports.mkdir()
    guide = data / "tv.plex.providers.epg.cloud-smoke.db"
    library = data / "com.plexapp.plugins.library.db"
    seed(guide, "Snapshot Fixture")
    seed(library, "Snapshot Fixture")
    (app / "pex-config.json").write_text(json.dumps({
        "plex_epg_db_source": str(guide),
        "plex_library_db_source": str(root / "absent-live-source.db"),
        "library_snapshot_dir": str(exports),
    }))
    publish = [sys.executable, str(app / "pex_snapshot.py"), str(library), str(exports)]
    subprocess.run(publish, check=True, timeout=60)
    launch(app, "Snapshot Fixture")
    mirror = app / "db/plex_library.db"
    stamp = mirror.stat().st_mtime_ns
    digest = hashlib.sha256(mirror.read_bytes()).digest()
    launch(app, "Snapshot Fixture")
    assert mirror.stat().st_mtime_ns == stamp
    assert hashlib.sha256(mirror.read_bytes()).digest() == digest
    with closing(sqlite3.connect(library)) as db:
        db.execute("UPDATE metadata_items SET title='New Generation', original_title='New Generation'")
        db.commit()
    subprocess.run(publish, check=True, timeout=60)
    launch(app, "New Generation")
    assert hashlib.sha256(mirror.read_bytes()).digest() != digest
    print("Portable snapshot smoke passed: installed, reused, then refreshed the library.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python tests/portable_snapshot_smoke.py <portable ZIP>")
    with tempfile.TemporaryDirectory(prefix="pex-snapshot-smoke-") as temp:
        smoke(Path(sys.argv[1]).resolve(), Path(temp).resolve())
