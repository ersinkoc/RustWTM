"""Add the website package to an existing stable release without replacing assets."""
import argparse
import hashlib
import io
import json
import re
import subprocess
import os
import urllib.request
from urllib.parse import urlparse
import zipfile
from pathlib import Path


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True, timeout=60)


def read_asset(asset, repository, tag, limit):
    url = urlparse(asset["browser_download_url"])
    expected = f"/{repository}/releases/download/{tag}/{asset['name']}"
    if url.scheme != "https" or url.netloc.lower() != "github.com" or url.path.lower() != expected.lower():
        raise ValueError("Unexpected public website asset URL")
    if not 0 < asset["size"] <= limit:
        raise ValueError("Oversized public website asset")
    with urllib.request.urlopen(asset["browser_download_url"], timeout=30) as response:
        data = response.read(limit + 1)
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    if len(data) != asset["size"] or asset.get("digest") != digest:
        raise ValueError("Existing website asset checksum mismatch")
    return data


def verify_existing(release, repository, tag, commit, reader=read_asset):
    names = ("rustwtm-website-media.zip", "rustwtm-website-media.zip.sha256")
    assets = [next((asset for asset in release["assets"] if asset["name"] == name), None) for name in names]
    if not any(assets):
        return False
    if not all(assets):
        raise ValueError("Existing website package is incomplete; refusing replacement")
    data = reader(assets[0], repository, tag, 64 * 1024 * 1024)
    checksum = reader(assets[1], repository, tag, 1024).decode("ascii").strip()
    if checksum != f"{hashlib.sha256(data).hexdigest()}  {names[0]}":
        raise ValueError("Existing website sidecar mismatch")
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        if len(entries) > 41 or len({entry.filename for entry in entries}) != len(entries):
            raise ValueError("Invalid existing website archive entries")
        if any(entry.file_size > 4 * 1024 * 1024 for entry in entries) or sum(entry.file_size for entry in entries) > 64 * 1024 * 1024:
            raise ValueError("Oversized existing website archive")
        manifest = json.loads(archive.read("manifest.json"))
        if manifest.get("schemaVersion") != 1 or manifest.get("repositoryId") != 1425760656 or manifest.get("version") != tag or manifest.get("sourceCommit") != commit:
            raise ValueError("Existing website package does not match the released source")
        captures = manifest.get("captures", [])
        if not 3 <= len(captures) <= 40:
            raise ValueError("Incomplete existing website gallery")
        ids = [capture["id"] for capture in captures]
        if len(set(ids)) != len(ids) or not {"processes", "performance", "overview"}.issubset(ids) or manifest.get("hero") not in ids:
            raise ValueError("Invalid existing website gallery")
        paths = {"manifest.json"}
        for capture in captures:
            expected_path = f"images/{capture['id']}.webp"
            if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", capture["id"]) or capture["path"] != expected_path:
                raise ValueError("Invalid existing website image path")
            image = archive.read(expected_path)
            if hashlib.sha256(image).hexdigest() != capture["sha256"] or image[:4] != b"RIFF" or image[8:12] != b"WEBP":
                raise ValueError("Existing website image checksum mismatch")
            paths.add(expected_path)
        if {entry.filename for entry in entries} != paths:
            raise ValueError("Unexpected existing website archive entries")
    print(f"Verified existing {tag} website package; preserving its published bytes.")
    return True


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--check-existing", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"v\d+\.\d+\.\d+", args.tag) or args.repository.lower() != "rustwtm/rustwtm":
        raise ValueError("Unexpected release destination")
    release = json.loads(gh("api", f"repos/{args.repository}/releases/tags/{args.tag}"))
    if release["draft"] or release["prerelease"] or release["tag_name"] != args.tag:
        raise ValueError("An already published stable release is required")
    ref = json.loads(gh("api", f"repos/{args.repository}/git/ref/tags/{args.tag}"))["object"]
    for _ in range(4):
        if ref["type"] == "commit":
            break
        if ref["type"] != "tag":
            raise ValueError("Unsupported release ref")
        ref = json.loads(gh("api", f"repos/{args.repository}/git/tags/{ref['sha']}"))["object"]
    if ref["type"] != "commit":
        raise ValueError("Unsupported release ref")
    existing = verify_existing(release, args.repository, args.tag, ref["sha"])
    if args.check_existing:
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
                output.write(f"complete={str(existing).lower()}\n")
        raise SystemExit(0)
    if existing:
        raise SystemExit(0)
    if not args.archive:
        parser.error("--archive is required when no complete package exists")
    with zipfile.ZipFile(args.archive) as archive:
        manifest = json.loads(archive.read("manifest.json"))
    if ref["sha"] != manifest["sourceCommit"] or manifest["version"] != args.tag:
        raise ValueError("Website package does not match the released source")
    for file in (args.archive, args.archive.with_suffix(args.archive.suffix + ".sha256")):
        digest = "sha256:" + hashlib.sha256(file.read_bytes()).hexdigest()
        existing = next((asset for asset in release["assets"] if asset["name"] == file.name), None)
        if existing:
            if existing.get("digest") != digest or existing["size"] != file.stat().st_size:
                raise ValueError(f"Refusing to replace an existing release asset: {file.name}")
        else:
            gh("release", "upload", args.tag, str(file), "--repo", args.repository)
        print(f"Verified public website asset: {file.name}")
