import importlib.util
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "make_portable" / "pex_snapshot.py"
spec = importlib.util.spec_from_file_location("pex_snapshot", SCRIPT)
exporter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exporter)


class SnapshotExportTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.source = Path(temp.name) / "Plex library #1.db"
        self.out = Path(temp.name) / "Exports for PEX"
        self.out.mkdir()
        with closing(sqlite3.connect(str(self.source))) as db:
            db.executescript(
                "CREATE TABLE metadata_items(id INTEGER PRIMARY KEY, title TEXT);"
                "CREATE TABLE media_items(id INTEGER PRIMARY KEY);"
                "CREATE TABLE media_parts(id INTEGER PRIMARY KEY, file TEXT);"
                "INSERT INTO metadata_items VALUES(1, 'first');"
                "INSERT INTO media_items VALUES(1);"
                "INSERT INTO media_parts VALUES(1, 'first.mkv');"
            )

    def test_wal_writer_commits_between_backup_steps(self):
        with closing(sqlite3.connect(str(self.source))) as writer:
            writer.execute("PRAGMA journal_mode=WAL")
            writer.execute("PRAGMA wal_autocheckpoint=0")
            writer.execute("CREATE TABLE padding(block BLOB)")
            writer.executemany("INSERT INTO padding VALUES(zeroblob(4096))", [()] * 600)
            writer.commit()
            real_connect = sqlite3.connect
            wrote = []
            out = self.out
            test = self

            class ObservedReader(sqlite3.Connection):
                def backup(self, target, *, pages, progress, sleep):
                    def during_copy(status, remaining, total):
                        if remaining and not wrote:
                            test.assertFalse(list(out.glob("*.db")))
                            writer.execute("UPDATE metadata_items SET title='second'")
                            writer.execute("UPDATE media_parts SET file='second.mkv'")
                            writer.commit()
                            wrote.append(True)
                        progress(status, remaining, total)

                    super().backup(target, pages=pages, progress=during_copy, sleep=sleep)

            def connect(path, **args):
                if args.get("uri"):
                    self.assertIn("mode=ro", path)
                    args["factory"] = ObservedReader
                return real_connect(path, **args)

            with mock.patch.object(exporter.sqlite3, "connect", side_effect=connect):
                result = exporter.export_snapshot(self.source, self.out)

            self.assertEqual(wrote, [True])
            self.assertEqual(writer.execute("SELECT title FROM metadata_items").fetchone(), ("second",))
            self.assertEqual(writer.execute("PRAGMA journal_mode").fetchone(), ("wal",))
            self.assertRegex(result.name, r"^pex-library-\d{4}-\d\d-\d\dT\d{12}Z\.db$")
            with closing(real_connect(result.as_uri() + "?mode=ro", uri=True)) as snapshot:
                self.assertEqual(snapshot.execute("SELECT title FROM metadata_items").fetchone(), ("first",))
                self.assertEqual(snapshot.execute("SELECT file FROM media_parts").fetchone(), ("first.mkv",))
                self.assertEqual(snapshot.execute("PRAGMA journal_mode").fetchone(), ("delete",))
                self.assertEqual(snapshot.execute("PRAGMA integrity_check").fetchone(), ("ok",))
            self.assertEqual(list(self.out.iterdir()), [result])

    def test_published_generation_cannot_be_clobbered(self):
        name = "pex-library-2025-02-15T120103123456Z.db"
        with mock.patch.object(exporter, "generation_name", return_value=name):
            result = exporter.export_snapshot(self.source, self.out)
            original = result.read_bytes()
            with closing(sqlite3.connect(str(self.source))) as db:
                db.execute("UPDATE metadata_items SET title='different'")
                db.commit()
            with self.assertRaises(FileExistsError):
                exporter.export_snapshot(self.source, self.out)
            self.assertEqual(result.read_bytes(), original)
        self.assertEqual(list(self.out.iterdir()), [result])

    def test_invalid_source_leaves_export_directory_untouched(self):
        unrelated = self.out / "another-task.tmp"
        unrelated.write_text("preserve")
        with closing(sqlite3.connect(str(self.source))) as db:
            db.execute("DROP TABLE media_parts")
        with self.assertRaises(ValueError):
            exporter.export_snapshot(self.source, self.out)
        self.assertEqual(list(self.out.iterdir()), [unrelated])

    def test_exclusive_lock_does_not_publish_or_modify_source(self):
        with closing(sqlite3.connect(str(self.source))) as writer:
            writer.execute("BEGIN EXCLUSIVE")
            with mock.patch.object(exporter, "BUSY_TIMEOUT_SECONDS", 0.1):
                with self.assertRaisesRegex(sqlite3.OperationalError, "locked"):
                    exporter.export_snapshot(self.source, self.out)
            self.assertTrue(writer.in_transaction)
            self.assertEqual(writer.execute("SELECT title FROM metadata_items").fetchone(), ("first",))
            writer.rollback()
        self.assertEqual(list(self.out.iterdir()), [])

    def test_cli_with_plex_collation_in_schema(self):
        with closing(sqlite3.connect(str(self.source))) as db:
            db.create_collation("icu_root", lambda a, b: (a > b) - (a < b))
            db.execute("CREATE INDEX movie_title ON metadata_items(title COLLATE icu_root)")
        run = subprocess.run(
            [sys.executable, str(SCRIPT), str(self.source), str(self.out)],
            capture_output=True, text=True,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        result = Path(run.stdout.strip())
        with closing(sqlite3.connect(result.as_uri() + "?mode=ro", uri=True)) as dst:
            self.assertEqual(dst.execute("SELECT title FROM metadata_items WHERE id=1").fetchone(), ("first",))
            schema = dst.execute("SELECT sql FROM sqlite_master WHERE name='movie_title'").fetchone()[0]
            self.assertIn("icu_root", schema)


if __name__ == "__main__":
    unittest.main()
