#!/usr/bin/env python3
"""Prepare versions/notes, package native binaries, and publish Beskar releases.

Python 3.11+, standard library only. Run from the repository root.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

TARGETS = (
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
)
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z")


def run(*args):
    return subprocess.check_output(args, text=True, encoding="utf-8").strip()


def version_tuple(value):
    if not VERSION.fullmatch(value):
        raise ValueError("Use a stable version X.Y.Z (without a v prefix).")
    return tuple(map(int, value.split(".")))


def workspace():
    return tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]


def resolve_version(requested, current):
    parts = list(version_tuple(current))
    if requested in ("major", "minor", "patch"):
        index = ("major", "minor", "patch").index(requested)
        parts[index] += 1
        parts[index + 1:] = [0] * (2 - index)
        return ".".join(map(str, parts))
    if version_tuple(requested) < tuple(parts):
        raise ValueError("Release version cannot go backwards.")
    return requested


def release_notes(version):
    path = Path(f"docs/releases/v{version}.md")
    body = path.read_text(encoding="utf-8").strip()
    if not body or re.search(r"\b(TODO|TBD)\b", body):
        raise ValueError(f"{path} needs finished release notes.")
    return body


def validate(tag):
    version = tag.removeprefix("v")
    version_tuple(version)
    if tag != f"v{version}" or workspace()["package"]["version"] != version:
        raise ValueError("Tag must match the workspace version (vX.Y.Z).")
    body = release_notes(version)
    changelog = Path("CHANGELOG.md").read_text(encoding="utf-8")
    match = re.search(rf"^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}\n(.*?)(?=^## \[|\Z)", changelog, re.M | re.S)
    if not match or match[1].strip() != body:
        raise ValueError("Release notes and the version's changelog entry must match.")
    lock = tomllib.loads(Path("Cargo.lock").read_text(encoding="utf-8"))
    names = {tomllib.loads((Path(member) / "Cargo.toml").read_text(encoding="utf-8"))["package"]["name"] for member in workspace()["members"]}
    locked = {p["name"]: p["version"] for p in lock["package"] if p["name"] in names and "source" not in p}
    if locked != dict.fromkeys(names, version):
        raise ValueError("Workspace package versions in Cargo.lock do not match.")
    return version


def prepare(requested, repository):
    if run("git", "status", "--porcelain"):
        raise ValueError("Commit your changes before preparing a release.")
    current = workspace()["package"]["version"]
    version = resolve_version(requested, current)
    tag = f"v{version}"
    if tag in run("git", "tag", "--list").splitlines():
        raise ValueError(f"{tag} already exists; rerun failed publication jobs or choose a new version.")
    path = Path(f"docs/releases/{tag}.md")
    if not path.exists():
        payload = {"tag_name": tag, "target_commitish": run("git", "rev-parse", "HEAD")}
        previous = run("git", "tag", "--merged", "HEAD", "--sort=-version:refname", "--list", "v*").splitlines()
        previous = [t for t in previous if VERSION.fullmatch(t.removeprefix("v"))]
        if previous:
            payload["previous_tag_name"] = previous[0]
        response = subprocess.run(
            ["gh", "api", f"repos/{repository}/releases/generate-notes", "--input", "-"],
            input=json.dumps(payload), text=True, encoding="utf-8", capture_output=True, check=True,
        )
        body = json.loads(response.stdout)["body"].strip()
        if not body:
            raise ValueError("GitHub generated empty notes; commit curated notes first.")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body + "\n", encoding="utf-8")
    body = release_notes(version)
    changelog_path = Path("CHANGELOG.md")
    changelog = changelog_path.read_text(encoding="utf-8") if changelog_path.exists() else "# Changelog\n\n"
    if re.search(rf"^## \[{re.escape(version)}\]", changelog, re.M):
        validate(tag)  # A previously prepared commit can be retried without another bump.
        return tag
    manifest_path = Path("Cargo.toml")
    manifest = manifest_path.read_text(encoding="utf-8")
    manifest, count = re.subn(r'(\[workspace\.package\]\s*\nversion = ")[^"]+(")', lambda m: m[1] + version + m[2], manifest, count=1)
    if count != 1:
        raise ValueError("Cannot find the workspace version assignment.")
    # Only rewrite local workspace packages. Registry packages and checksums remain intact.
    names = {tomllib.loads((Path(m) / "Cargo.toml").read_text(encoding="utf-8"))["package"]["name"] for m in workspace()["members"]}
    lock_path = Path("Cargo.lock")
    blocks = lock_path.read_text(encoding="utf-8").split("[[package]]")
    updated = []
    for block in blocks[1:]:
        package = tomllib.loads(block)
        if package["name"] in names and "source" not in package:
            block = re.sub(r'^version = "[^"]+"', f'version = "{version}"', block, count=1, flags=re.M)
        updated.append(block)
    date = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
    entry = f"## [{version}] - {date}\n\n{body}\n\n"
    if not changelog.startswith("# Changelog\n"):
        raise ValueError("CHANGELOG.md must start with '# Changelog'.")
    manifest_path.write_text(manifest, encoding="utf-8")
    lock_path.write_text("[[package]]".join([blocks[0], *updated]), encoding="utf-8")
    changelog_path.write_text("# Changelog\n\n" + entry + changelog[len("# Changelog\n"):].lstrip(), encoding="utf-8")
    validate(tag)
    return tag


def package(target, tag, output):
    version = validate(tag)
    if target not in TARGETS:
        raise ValueError(f"Unsupported release target: {target}")
    suffix = ".exe" if "windows" in target else ""
    binaries = [Path("target") / target / "release" / (name + suffix) for name in ("beskar", "beskar-gui")]
    for binary in binaries:
        if not binary.is_file():
            raise ValueError(f"Missing binary: {binary}")
    output.mkdir(parents=True, exist_ok=True)
    stem = f"beskar-v{version}-{target}"
    with tempfile.TemporaryDirectory() as temporary:
        staging = Path(temporary) / stem
        staging.mkdir()
        for binary in binaries:
            shutil.copy2(binary, staging / binary.name)
            if not suffix:
                (staging / binary.name).chmod(0o755)
        for source in (Path("README.md"), Path("CHANGELOG.md"), Path("docs/RELEASING.md")):
            shutil.copy2(source, staging / source.name)
        if suffix:
            archive = output / f"{stem}.zip"
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as handle:
                for source in sorted(staging.iterdir()):
                    handle.write(source, f"{stem}/{source.name}")
        else:
            archive = output / f"{stem}.tar.gz"
            def permissions(member):
                # Windows chmod cannot set Unix executable bits. Set archive
                # metadata explicitly so packaging also works on that host.
                executable = Path(member.name).name in ("beskar", "beskar-gui")
                member.mode = 0o755 if member.isdir() or executable else 0o644
                return member
            with tarfile.open(archive, "w:gz") as handle:
                handle.add(staging, arcname=stem, filter=permissions)
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return archive


def publish(tag, repository, artifacts):
    version = validate(tag)
    if run("git", "rev-parse", f"{tag}^{{commit}}") != run("git", "rev-parse", "HEAD"):
        raise ValueError("Release tag does not point at this checkout.")
    expected = {f"beskar-{tag}-{t}" + (".zip" if "windows" in t else ".tar.gz") for t in TARGETS}
    if {p.name for p in artifacts.iterdir()} != expected | {n + ".sha256" for n in expected}:
        raise ValueError("Expected exactly four native archives and their checksum files.")
    for name in expected:
        archive = artifacts / name
        with archive.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if archive.with_name(name + ".sha256").read_text(encoding="utf-8") != f"{digest}  {name}\n":
            raise ValueError(f"Checksum mismatch: {name}")
    existing = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{repository}/releases?per_page=100"))
    release = next((r for page in existing for r in page if r["tag_name"] == tag), None)
    if release and not release["draft"]:
        raise ValueError(f"{tag} is already published; refusing to replace its assets.")
    notes = str(Path(f"docs/releases/{tag}.md"))
    if not release:
        run("gh", "release", "create", tag, "--repo", repository, "--verify-tag", "--draft", "--title", f"Beskar {version}", "--notes-file", notes)
    run("gh", "release", "upload", tag, "--repo", repository, "--clobber", *map(str, sorted(artifacts.iterdir())))
    run("gh", "release", "edit", tag, "--repo", repository, "--notes-file", notes, "--draft=false", "--latest")
    print(f"https://github.com/{repository}/releases/tag/{tag}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prep = commands.add_parser("prepare", help="Update version, lockfile and changelog; does not commit or push")
    prep.add_argument("--version", required=True, help="X.Y.Z, major, minor or patch")
    prep.add_argument("--repository", default="ventrion/beskar")
    check = commands.add_parser("validate")
    check.add_argument("--tag", required=True)
    pack = commands.add_parser("package")
    pack.add_argument("--tag", required=True)
    pack.add_argument("--target", required=True, choices=TARGETS)
    pack.add_argument("--output", type=Path, default=Path("dist"))
    pub = commands.add_parser("publish")
    pub.add_argument("--tag", required=True)
    pub.add_argument("--repository", default="ventrion/beskar")
    pub.add_argument("--artifacts", type=Path, default=Path("dist"))
    args = parser.parse_args()
    if args.command == "prepare":
        tag = prepare(args.version, args.repository)
        print(tag)
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
                stream.write(f"tag={tag}\n")
    elif args.command == "validate":
        print(validate(args.tag))
    elif args.command == "package":
        print(package(args.target, args.tag, args.output))
    else:
        publish(args.tag, args.repository, args.artifacts)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
