"""Add the website package to an existing stable release without replacing assets."""
import argparse
import hashlib
import json
import re
import subprocess
import zipfile
from pathlib import Path


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True, timeout=60)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--archive", type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"v\d+\.\d+\.\d+", args.tag) or args.repository.lower() != "rustwtm/rustwtm":
        raise ValueError("Unexpected release destination")
    release = json.loads(gh("api", f"repos/{args.repository}/releases/tags/{args.tag}"))
    if release["draft"] or release["prerelease"] or release["tag_name"] != args.tag:
        raise ValueError("An already published stable release is required")
    with zipfile.ZipFile(args.archive) as archive:
        manifest = json.loads(archive.read("manifest.json"))
    ref = json.loads(gh("api", f"repos/{args.repository}/git/ref/tags/{args.tag}"))["object"]
    for _ in range(4):
        if ref["type"] == "commit":
            break
        if ref["type"] != "tag":
            raise ValueError("Unsupported release ref")
        ref = json.loads(gh("api", f"repos/{args.repository}/git/tags/{ref['sha']}"))["object"]
    if ref["type"] != "commit" or ref["sha"] != manifest["sourceCommit"] or manifest["version"] != args.tag:
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
