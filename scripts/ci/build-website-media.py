"""Package exact-tag, fixture-rendered GUI captures for the public website."""
import argparse
import hashlib
import io
import json
import re
import subprocess
import tomllib
import zipfile
from pathlib import Path

from PIL import Image

Image.MAX_IMAGE_PIXELS = 4096 * 4096
ASSET = "rustwtm-website-media.zip"
REPOSITORY_ID = 1425760656


def slug(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "-", name).lower()


def build(product, renders, tag, source, output):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag) or not re.fullmatch(r"[a-f0-9]{40}", source):
        raise ValueError("A stable release tag and exact source commit are required")
    cargo = tomllib.loads((product / "Cargo.toml").read_text(encoding="utf-8"))
    if "v" + cargo["workspace"]["package"]["version"] != tag:
        raise ValueError("The rendered product version does not match the release")
    app = (product / "crates/wtm-gui/src/app.rs").read_text(encoding="utf-8")
    enum = re.search(r"enum Tab\s*\{([^}]+)\}", app)
    if not enum:
        raise ValueError("The product navigation contract was not found")
    tabs = re.findall(r"^\s*(\w+)\s*,", enum[1], re.M)
    labels = dict(re.findall(r'\(Tab::(\w+),\s*I::\w+,\s*"([^"]+)"\)', app))
    aliases = {"InstalledApps": "installed-apps", "History": "app-history", "KillList": "kill-list", "Hardware": "hardware-specifications"}
    files = {}
    for tab in tabs:
        name = aliases.get(tab, slug(tab))
        files[name] = (f"visual-review/locales/en/dark-wide-{tab}.png", labels.get(tab, tab))
    files.update({
        "processes-light": ("visual-review/locales/en/light-wide-Processes.png", "Light theme"),
        "column-layouts": ("visual-review/locales/en/dark-wide-ColumnEditor.png", "Column layouts"),
        "process-inspection": ("visual-review/locales/en/dark-wide-ProcessInspector.png", "Process inspection"),
        "performance-timeline": ("visual-review/locales/en/dark-wide-PerformanceInspect.png", "Performance timeline"),
        "quick-setup": ("onboarding-review/welcome-light-4.png", "Quick setup"),
        "device-disk-details": ("product-expansion-review/en-dark-wide-disk-details.png", "Disk health"),
        "device-gpu-overview": ("product-expansion-review/en-light-wide-gpu.png", "GPU devices"),
    })
    if not {"processes", "performance", "overview"}.issubset(files) or not 3 <= len(tabs) <= 32:
        raise ValueError("Incomplete or unbounded product navigation")
    order = ["processes", "quick-setup", "column-layouts", "processes-light", "performance", "device-disk-details", "device-gpu-overview"]
    names = sorted(files, key=lambda name: (order.index(name) if name in order else len(order), list(files).index(name)))
    captures, payloads = [], {}
    for name in names:
        source_path, label = files[name]
        image_path = renders / source_path
        if not image_path.is_file() or image_path.stat().st_size > 16 * 1024 * 1024:
            raise ValueError(f"Missing or oversized exact-release capture: {source_path}")
        with Image.open(image_path) as image:
            if image.format != "PNG" or not (1 <= image.width <= 4096 and 1 <= image.height <= 4096):
                raise ValueError(f"Invalid product capture: {source_path}")
            image.load()
            encoded = io.BytesIO()
            image.convert("RGB").save(encoded, format="WEBP", quality=90, method=6)
            payload = encoded.getvalue()
            width, height = image.size
        image_name = f"images/{name}.webp"
        payloads[image_name] = payload
        color = "memory" if any(word in name for word in ("memory", "performance", "gpu", "limits")) else "disk" if "disk" in name or "sensor" in name else "network" if "network" in name or "port" in name or "container" in name else "accent"
        captures.append({"id": name, "label": label, "title": label, "description": f"The {label.lower()} workspace in RustWTM {tag}. Actual application render with example data.", "color": color, "path": image_name, "sha256": hashlib.sha256(payload).hexdigest(), "width": width, "height": height, "source": source_path})
    manifest = {"schemaVersion": 1, "repositoryId": REPOSITORY_ID, "version": tag, "sourceCommit": source, "hero": "performance", "captures": captures}
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as archive:
        payloads["manifest.json"] = (json.dumps(manifest, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode()
        for name, payload in sorted(payloads.items()):
            entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, payload)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    output.with_suffix(output.suffix + ".sha256").write_text(f"{digest}  {output.name}\n", encoding="ascii")
    print(f"Packaged {len(captures)} {tag} captures from {source}: {digest}")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--product", type=Path, required=True)
    parser.add_argument("--renders", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=args.product, text=True, timeout=15).strip()
    build(args.product, args.renders, args.tag, source, args.output)
