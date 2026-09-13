"""Exercise version/lockfile integrity, recovery, and distributable contents."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.previous = Path.cwd()
        os.chdir(self.temp.name)
        self.addCleanup(self.cleanup)
        self.write("Cargo.toml", '[workspace]\nmembers = ["crates/one", "crates/two"]\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n')
        for name in ("one", "two"):
            self.write(f"crates/{name}/Cargo.toml", f'[package]\nname = "{name}"\nversion.workspace = true\n')
        self.write("Cargo.lock", 'version = 4\n\n[[package]]\nname = "one"\nversion = "0.1.0"\n\n[[package]]\nname = "two"\nversion = "0.1.0"\n\n[[package]]\nname = "external"\nversion = "1.2.3"\nsource = "registry+https://example.com"\nchecksum = "unchanged"\n')
        self.write("README.md", "Beskar\n")
        self.write("docs/RELEASING.md", "Release instructions\n")
        self.write(".gitignore", "target/\ndist/\n")
        release.run("git", "init", "--quiet")
        release.run("git", "config", "user.name", "Release Test")
        release.run("git", "config", "user.email", "release@example.com")
        self.commit()

    def cleanup(self):
        os.chdir(self.previous)
        self.temp.cleanup()

    def write(self, filename, body):
        path = Path(filename)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")

    def commit(self):
        release.run("git", "add", ".")
        release.run("git", "commit", "--quiet", "-m", "Fixture")

    def prepare(self, version="0.2.0"):
        self.write(f"docs/releases/v{version}.md", "### Features\n\n- Supports café profiles.\n")
        self.commit()
        return release.prepare(version, "ventrion/beskar")

    def test_bump_updates_all_local_versions_and_preserves_registry(self):
        self.assertEqual(self.prepare(), "v0.2.0")
        self.assertEqual(release.validate("v0.2.0"), "0.2.0")
        self.assertIn('name = "external"\nversion = "1.2.3"\nsource = "registry+https://example.com"\nchecksum = "unchanged"', Path("Cargo.lock").read_text())
        self.assertEqual(release.run("git", "tag", "--list"), "")

    def test_initial_release_can_keep_workspace_version(self):
        self.assertEqual(self.prepare("0.1.0"), "v0.1.0")
        release.validate("v0.1.0")

    def test_prepared_release_retry_keeps_one_changelog_entry(self):
        self.prepare()
        self.commit()
        original = Path("CHANGELOG.md").read_bytes()
        self.assertEqual(release.prepare("0.2.0", "ventrion/beskar"), "v0.2.0")
        self.assertEqual(Path("CHANGELOG.md").read_bytes(), original)

    def test_new_release_retains_old_changelog_entry(self):
        self.prepare("0.1.0")
        self.commit()
        self.prepare("0.2.0")
        body = Path("CHANGELOG.md").read_text()
        self.assertLess(body.index("## [0.2.0]"), body.index("## [0.1.0]"))
        release.validate("v0.2.0")

    def test_dirty_tree_and_existing_tag_rejected(self):
        self.write("dirty.txt", "pending user change")
        with self.assertRaisesRegex(ValueError, "Commit your changes"):
            release.prepare("0.2.0", "ventrion/beskar")
        self.commit()
        release.run("git", "tag", "v0.2.0")
        with self.assertRaisesRegex(ValueError, "already exists"):
            release.prepare("0.2.0", "ventrion/beskar")

    def test_invalid_and_decreasing_versions_rejected(self):
        for version in ("v1.0.0", "01.0.0", "1.0.0-rc.1", "0.0.9", "$(whoami)", "1.0.0\n"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                release.resolve_version(version, "0.1.0")
        for kind, expected in (("major", "2.0.0"), ("minor", "1.3.0"), ("patch", "1.2.4")):
            self.assertEqual(release.resolve_version(kind, "1.2.3"), expected)

    def test_validation_rejects_tag_notes_and_lock_drift(self):
        self.prepare()
        with self.assertRaisesRegex(ValueError, "Tag must match"):
            release.validate("v0.3.0")
        self.write("docs/releases/v0.2.0.md", "different notes")
        with self.assertRaisesRegex(ValueError, "must match"):
            release.validate("v0.2.0")
        self.write("docs/releases/v0.2.0.md", "### Features\n\n- Supports café profiles.\n")
        self.write("Cargo.lock", Path("Cargo.lock").read_text().replace('version = "0.2.0"', 'version = "0.1.0"', 1))
        with self.assertRaisesRegex(ValueError, "Cargo.lock"):
            release.validate("v0.2.0")

    def test_generated_notes_use_previous_tag_and_exact_commit(self):
        release.run("git", "tag", "v0.0.1")
        original = subprocess.run
        requests = []
        def api(args, **kwargs):
            if args[0] == "gh":
                requests.append(json.loads(kwargs["input"]))
                return subprocess.CompletedProcess(args, 0, json.dumps({"body": "### Changes\n\n- Generated PR notes."}))
            return original(args, **kwargs)
        with patch.object(release.subprocess, "run", side_effect=api):
            release.prepare("patch", "ventrion/beskar")
        self.assertEqual(requests[0]["tag_name"], "v0.1.1")
        self.assertEqual(requests[0]["previous_tag_name"], "v0.0.1")
        self.assertEqual(requests[0]["target_commitish"], release.run("git", "rev-parse", "HEAD"))
        release.validate("v0.1.1")

    def make_archives(self):
        self.prepare()
        for target in release.TARGETS:
            suffix = ".exe" if "windows" in target else ""
            for name in ("beskar", "beskar-gui"):
                self.write(f"target/{target}/release/{name}{suffix}", "fixture executable\n")
            release.package(target, "v0.2.0", Path("dist"))

    def test_archive_contents_permissions_and_checksums(self):
        # Windows ignores chmod's execute bits; tar headers must still be valid.
        with patch.object(Path, "chmod"):
            self.make_archives()
        self.assertEqual(len(list(Path("dist").iterdir())), 8)
        for target in release.TARGETS:
            stem = f"beskar-v0.2.0-{target}"
            if "windows" in target:
                path = Path(f"dist/{stem}.zip")
                with zipfile.ZipFile(path) as archive:
                    self.assertIn(f"{stem}/beskar.exe", archive.namelist())
                    self.assertIn(f"{stem}/beskar-gui.exe", archive.namelist())
            else:
                path = Path(f"dist/{stem}.tar.gz")
                with tarfile.open(path) as archive:
                    for binary in ("beskar", "beskar-gui"):
                        self.assertEqual(archive.getmember(f"{stem}/{binary}").mode & 0o777, 0o755)
                    self.assertIn(f"{stem}/CHANGELOG.md", archive.getnames())
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            self.assertEqual(Path(str(path) + ".sha256").read_text(), f"{digest}  {path.name}\n")

    def test_package_rejects_missing_binary(self):
        self.prepare()
        with self.assertRaisesRegex(ValueError, "Missing binary"):
            release.package(release.TARGETS[0], "v0.2.0", Path("dist"))

    def test_publish_validates_assets_and_draft_then_publishes(self):
        self.make_archives()
        calls = []
        def gh(*args):
            calls.append(args)
            if args[0] == "git":
                return "same-commit"
            if args[1] == "api":
                return "[[]]"
            return ""
        with patch.object(release, "run", side_effect=gh):
            release.publish("v0.2.0", "ventrion/beskar", Path("dist"))
        mutations = [args for args in calls if args[:2] == ("gh", "release")]
        self.assertEqual([args[2] for args in mutations], ["create", "upload", "edit"])
        self.assertIn("--draft", mutations[0])
        self.assertIn("--draft=false", mutations[-1])
        next(Path("dist").glob("*.sha256")).write_text("bad checksum")
        with patch.object(release, "run", return_value="same-commit"), self.assertRaisesRegex(ValueError, "Checksum mismatch"):
            release.publish("v0.2.0", "ventrion/beskar", Path("dist"))

    def test_publish_refuses_incomplete_or_published_release(self):
        self.make_archives()
        with patch.object(release, "run", side_effect=lambda *a: "same" if a[0] == "git" else '[[{"tag_name":"v0.2.0","draft":false}]]'), self.assertRaisesRegex(ValueError, "already published"):
            release.publish("v0.2.0", "ventrion/beskar", Path("dist"))
        next(Path("dist").glob("*.zip")).unlink()
        with patch.object(release, "run", return_value="same"), self.assertRaisesRegex(ValueError, "exactly four"):
            release.publish("v0.2.0", "ventrion/beskar", Path("dist"))


if __name__ == "__main__":
    unittest.main()
