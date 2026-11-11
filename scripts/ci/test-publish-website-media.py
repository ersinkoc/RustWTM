import hashlib
import importlib.util
import io
import json
import runpy
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("publish-website-media.py")
spec = importlib.util.spec_from_file_location("publisher", SCRIPT)
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


def fixture(version="v1.2.3", source="a" * 40, corrupt_image=False):
    image = b"RIFF\x10\x00\x00\x00WEBPfixture"
    captures = [{"id": name, "path": f"images/{name}.webp", "sha256": hashlib.sha256(image).hexdigest(), "width": 2, "height": 2} for name in ("processes", "performance", "overview")]
    manifest = {"schemaVersion": 1, "repositoryId": 1425760656, "version": version, "sourceCommit": source, "hero": "performance", "captures": captures}
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        archive.writestr("manifest.json", json.dumps(manifest))
        for capture in captures:
            archive.writestr(capture["path"], image + (b"corrupted" if corrupt_image else b""))
    data = output.getvalue()
    checksum = f"{hashlib.sha256(data).hexdigest()}  rustwtm-website-media.zip\n".encode()
    blobs = [data, checksum]
    assets = [{"name": name, "size": len(blob), "digest": "sha256:" + hashlib.sha256(blob).hexdigest(), "browser_download_url": f"https://github.com/RustWTM/RustWTM/releases/download/v1.2.3/{name}"} for name, blob in zip(("rustwtm-website-media.zip", "rustwtm-website-media.zip.sha256"), blobs)]
    return {"draft": False, "prerelease": False, "tag_name": "v1.2.3", "assets": assets}, blobs


class PublisherTests(unittest.TestCase):
    def verify(self, release, blobs):
        with patch("urllib.request.urlopen", side_effect=[io.BytesIO(blob) for blob in blobs]):
            return publisher.verify_existing(release, "RustWTM/RustWTM", "v1.2.3", "a" * 40)

    def test_same_source_rerender_preserves_valid_published_package(self):
        release, blobs = fixture()
        with tempfile.TemporaryDirectory() as directory:
            rerender = Path(directory) / "rustwtm-website-media.zip"
            with zipfile.ZipFile(rerender, "w") as archive:
                archive.writestr("manifest.json", json.dumps({"version": "v1.2.3", "sourceCommit": "a" * 40}))
            self.assertNotEqual(hashlib.sha256(rerender.read_bytes()).hexdigest(), release["assets"][0]["digest"][7:])
            with patch("sys.argv", [str(SCRIPT), "--repository", "RustWTM/RustWTM", "--tag", "v1.2.3", "--archive", str(rerender)]), patch("subprocess.check_output", side_effect=[json.dumps(release), json.dumps({"object": {"type": "commit", "sha": "a" * 40}})]) as gh, patch("urllib.request.urlopen", side_effect=[io.BytesIO(blob) for blob in blobs]):
                with self.assertRaises(SystemExit) as result:
                    runpy.run_path(str(SCRIPT), run_name="__main__")
                self.assertEqual(result.exception.code, 0)
                self.assertEqual(gh.call_count, 2, "No upload or replacement is allowed")

    def test_valid_existing_package_and_absent_package_controls(self):
        self.assertTrue(self.verify(*fixture()))
        self.assertFalse(self.verify({"assets": []}, []))

    def test_original_published_bytes_are_a_passing_cli_control(self):
        release, blobs = fixture()
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "rustwtm-website-media.zip"
            archive.write_bytes(blobs[0])
            archive.with_suffix(".zip.sha256").write_bytes(blobs[1])
            with patch("sys.argv", [str(SCRIPT), "--repository", "RustWTM/RustWTM", "--tag", "v1.2.3", "--archive", str(archive)]), patch("subprocess.check_output", side_effect=[json.dumps(release), json.dumps({"object": {"type": "commit", "sha": "a" * 40}})]) as gh, patch("urllib.request.urlopen", side_effect=[io.BytesIO(blob) for blob in blobs]):
                try:
                    runpy.run_path(str(SCRIPT), run_name="__main__")
                except SystemExit as result:
                    self.assertEqual(result.code, 0)
                self.assertEqual(gh.call_count, 2)

    def test_wrong_version_and_commit_are_rejected(self):
        for version, source in (("v9.9.9", "a" * 40), ("v1.2.3", "b" * 40)):
            with self.subTest(version=version, source=source), self.assertRaisesRegex(ValueError, "released source"):
                self.verify(*fixture(version, source))

    def test_corrupt_images_and_archive_digest_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "image checksum"):
            self.verify(*fixture(corrupt_image=True))
        release, blobs = fixture()
        release["assets"][0]["digest"] = "sha256:" + "f" * 64
        with self.assertRaisesRegex(ValueError, "asset checksum"):
            self.verify(release, blobs)

    def test_incomplete_package_cannot_be_replaced(self):
        release, blobs = fixture()
        release["assets"].pop()
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.verify(release, blobs)

    def test_foreign_download_origin_is_rejected(self):
        release, blobs = fixture()
        release["assets"][0]["browser_download_url"] = "https://example.invalid/media.zip"
        with self.assertRaisesRegex(ValueError, "Unexpected public"):
            self.verify(release, blobs)


if __name__ == "__main__":
    unittest.main()
