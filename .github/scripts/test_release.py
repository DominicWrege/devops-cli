import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    def test_tag_must_match_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / "Cargo.toml"
            manifest.write_text('[package]\nversion = "0.1.0"\n', encoding="utf-8")
            self.assertEqual(release.validate("v0.1.0", manifest), "0.1.0")
            for tag in ["0.1.0", "v0.2.0", "v0.1.0/bad", "v0.1.0\n"]:
                with self.assertRaises(ValueError):
                    release.validate(tag, manifest)
            manifest.write_text('[package]\nversion = "0.2.0-rc.1"\n', encoding="utf-8")
            self.assertEqual(release.validate("v0.2.0-rc.1", manifest), "0.2.0-rc.1")

    def test_archives_include_binary_notice_and_checksum(self):
        for target, executable in release.TARGETS.items():
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "README.md").write_text("readme", encoding="utf-8")
                (root / "LICENSES").mkdir()
                (root / "LICENSES" / "az-mise-plugin.txt").write_text("notice", encoding="utf-8")
                binary = root / executable
                binary.write_bytes(b"fixture executable")
                archive = release.package("0.1.0", target, binary, root / "dist", root)
                prefix = f"devops-v0.1.0-{target}"
                if target.endswith("msvc"):
                    with zipfile.ZipFile(archive) as package:
                        files = package.namelist()
                        self.assertEqual(package.read(f"{prefix}/{executable}"), binary.read_bytes())
                else:
                    with tarfile.open(archive) as package:
                        files = package.getnames()
                        self.assertEqual(package.getmember(f"{prefix}/{executable}").mode, 0o755)
                        self.assertEqual(package.extractfile(f"{prefix}/{executable}").read(), binary.read_bytes())
                self.assertIn(f"{prefix}/README.md", files)
                self.assertIn(f"{prefix}/LICENSES/az-mise-plugin.txt", files)
                expected = hashlib.sha256(archive.read_bytes()).hexdigest()
                checksum = archive.with_name(archive.name + ".sha256").read_text(encoding="utf-8")
                self.assertEqual(checksum, f"{expected}  {archive.name}\n")


if __name__ == "__main__":
    unittest.main()
