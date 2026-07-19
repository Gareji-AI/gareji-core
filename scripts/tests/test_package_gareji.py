from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
import zipfile


SCRIPT = Path(__file__).resolve().parents[1] / "package_gareji.py"
SPEC = importlib.util.spec_from_file_location("package_gareji", SCRIPT)
assert SPEC and SPEC.loader
package_gareji = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = package_gareji
SPEC.loader.exec_module(package_gareji)


class PackageGarejiTests(unittest.TestCase):
    def fixture(self, root: Path, target: str) -> tuple[Path, Path, Path]:
        repo = root / "repo"
        release = root / "release"
        output = root / "dist"
        (repo / ".agents" / "plugins").mkdir(parents=True)
        plugin = repo / "plugins" / "gareji-progress"
        (plugin / ".codex-plugin").mkdir(parents=True)
        (plugin / "scripts" / "__pycache__").mkdir(parents=True)
        (plugin / "tests").mkdir()
        release.mkdir()
        (repo / "Cargo.toml").write_text(
            '[workspace]\n[workspace.package]\nversion = "1.2.3"\n', encoding="utf-8"
        )
        (repo / ".agents" / "plugins" / "marketplace.json").write_text(
            '{"name":"gareji-local"}\n', encoding="utf-8"
        )
        (plugin / ".codex-plugin" / "plugin.json").write_text(
            '{"name":"gareji-progress"}\n', encoding="utf-8"
        )
        (plugin / "scripts" / "record_stop.py").write_text("pass\n", encoding="utf-8")
        (plugin / "scripts" / "__pycache__" / "record_stop.pyc").write_bytes(b"cache")
        (plugin / "tests" / "test_record_stop.py").write_text("pass\n", encoding="utf-8")
        (repo / "README.md").write_text("# Gareji\n", encoding="utf-8")
        for name in (
            package_gareji.executable_name("gareji", target),
            package_gareji.executable_name("gareji-core", target),
        ):
            (release / name).write_bytes(b"binary")
        return repo, release, output

    def test_windows_bundle_contains_runtime_files_and_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = "x86_64-pc-windows-msvc"
            repo, release, output = self.fixture(root, target)

            archive = package_gareji.package_distribution(repo, release, output, target)

            self.assertEqual(archive.suffix, ".zip")
            self.assertEqual(archive.name, "gareji-1.2.3-x86_64-pc-windows-msvc.zip")
            self.assertTrue(archive.with_name(f"{archive.name}.sha256").is_file())
            with zipfile.ZipFile(archive) as bundled:
                names = set(bundled.namelist())
            prefix = "gareji-1.2.3-x86_64-pc-windows-msvc/"
            self.assertIn(prefix + "gareji.exe", names)
            self.assertIn(prefix + "gareji-core.exe", names)
            self.assertIn(prefix + "manifest.json", names)
            self.assertIn(prefix + "plugins/gareji-progress/scripts/record_stop.py", names)
            self.assertFalse(any("__pycache__" in name for name in names))
            self.assertFalse(any("/tests/" in name for name in names))

    def test_unix_bundle_is_tarball_with_a_complete_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = "x86_64-unknown-linux-gnu"
            repo, release, output = self.fixture(root, target)

            archive = package_gareji.package_distribution(repo, release, output, target)

            self.assertTrue(archive.name.endswith(".tar.gz"))
            with tarfile.open(archive, "r:gz") as bundled:
                names = set(bundled.getnames())
            prefix = "gareji-1.2.3-x86_64-unknown-linux-gnu/"
            self.assertIn(prefix + "gareji", names)
            self.assertIn(prefix + "gareji-core", names)
            manifest = json.loads((output / prefix / "manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["format"], "gareji-distribution-v0")
            self.assertEqual(manifest["version"], "1.2.3")
            self.assertGreaterEqual(len(manifest["files"]), 5)


if __name__ == "__main__":
    unittest.main()
