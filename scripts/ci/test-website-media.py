import importlib.util
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

from PIL import Image

spec = importlib.util.spec_from_file_location("website_media", Path(__file__).with_name("build-website-media.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class WebsiteMediaTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.product, self.renders = self.root / "product", self.root / "renders"
        self.app = self.product / "crates/wtm-gui/src/app.rs"
        self.app.parent.mkdir(parents=True)
        self.app.write_text("enum Tab {\nProcesses,\nPerformance,\nOverview,\n}\n", encoding="utf-8")
        (self.product / "Cargo.toml").write_text('[workspace.package]\nversion="1.2.3"\n', encoding="utf-8")
        files = [f"visual-review/locales/en/dark-wide-{name}.png" for name in ("Processes", "Performance", "Overview", "ColumnEditor", "ProcessInspector", "PerformanceInspect")]
        files += ["visual-review/locales/en/light-wide-Processes.png", "onboarding-review/welcome-light-4.png", "product-expansion-review/en-dark-wide-disk-details.png", "product-expansion-review/en-light-wide-gpu.png"]
        for name in files:
            file = self.renders / name
            file.parent.mkdir(parents=True, exist_ok=True)
            Image.new("RGB", (16, 12), "#213547").save(file)

    def build(self, output="site.zip"):
        return module.build(self.product, self.renders, "v1.2.3", "a" * 40, self.root / output)

    def test_deterministic_package_contains_only_versioned_webp_captures(self):
        manifest = self.build()
        self.build("again.zip")
        self.assertEqual((self.root / "site.zip").read_bytes(), (self.root / "again.zip").read_bytes())
        self.assertEqual(manifest["version"], "v1.2.3")
        self.assertEqual(manifest["sourceCommit"], "a" * 40)
        self.assertEqual(len(manifest["captures"]), 10)
        with zipfile.ZipFile(self.root / "site.zip") as archive:
            self.assertEqual(set(archive.namelist()), {"manifest.json", *(capture["path"] for capture in manifest["captures"])})
            self.assertEqual(json.loads(archive.read("manifest.json")), manifest)
            for capture in manifest["captures"]:
                self.assertEqual(capture["width"], 16)
                self.assertTrue(archive.read(capture["path"]).startswith(b"RIFF"))

    def test_missing_capture_cannot_publish_an_incomplete_package(self):
        (self.renders / "visual-review/locales/en/dark-wide-Processes.png").unlink()
        with self.assertRaisesRegex(ValueError, "Missing or oversized"):
            self.build()
        self.assertFalse((self.root / "site.zip").exists())

    def test_product_version_must_match_the_requested_release(self):
        with self.assertRaisesRegex(ValueError, "version does not match"):
            module.build(self.product, self.renders, "v9.9.9", "a" * 40, self.root / "site.zip")

    def test_new_navigation_page_requires_its_own_render(self):
        self.app.write_text("enum Tab {\nProcesses,\nPerformance,\nOverview,\nNewPage,\n}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "dark-wide-NewPage.png"):
            self.build()
        image = self.renders / "visual-review/locales/en/dark-wide-NewPage.png"
        Image.new("RGB", (16, 12), "blue").save(image)
        self.assertIn("new-page", [capture["id"] for capture in self.build()["captures"]])


if __name__ == "__main__":
    unittest.main()
