#!/usr/bin/env python3
"""Build a self-contained Gareji distribution archive."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile


WINDOWS_TARGET_MARKER = "windows"
RUNTIME_EXCLUDES = {"__pycache__", "tests"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Rust target triple")
    parser.add_argument(
        "--release-dir",
        type=Path,
        required=True,
        help="Directory containing the release binaries",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("dist"),
        help="Directory for the staging tree and archive",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--smoke-test",
        action="store_true",
        help="Exercise the staged binaries and a dry-run setup before archiving",
    )
    return parser.parse_args()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def workspace_version(repo_root: Path) -> str:
    with (repo_root / "Cargo.toml").open("rb") as stream:
        manifest = tomllib.load(stream)
    return str(manifest["workspace"]["package"]["version"])


def executable_name(name: str, target: str) -> str:
    return f"{name}.exe" if WINDOWS_TARGET_MARKER in target else name


def safe_clean_staging(staging: Path, output_dir: Path) -> None:
    staging = staging.resolve()
    output_dir = output_dir.resolve()
    if staging.parent != output_dir or not staging.name.startswith("gareji-"):
        raise ValueError(f"refusing to clean unexpected staging path: {staging}")
    if staging.exists():
        shutil.rmtree(staging)


def ignore_plugin_runtime(_directory: str, names: list[str]) -> set[str]:
    ignored = {name for name in names if name in RUNTIME_EXCLUDES}
    ignored.update(name for name in names if name.endswith((".pyc", ".pyo")))
    return ignored


def stage_distribution(
    repo_root: Path, release_dir: Path, output_dir: Path, target: str
) -> Path:
    repo_root = repo_root.resolve()
    release_dir = release_dir.resolve()
    output_dir = output_dir.resolve()
    version = workspace_version(repo_root)
    staging = output_dir / f"gareji-{version}-{target}"

    output_dir.mkdir(parents=True, exist_ok=True)
    safe_clean_staging(staging, output_dir)
    staging.mkdir()

    binaries = [executable_name("gareji", target), executable_name("gareji-core", target)]
    for binary in binaries:
        source = release_dir / binary
        if not source.is_file():
            raise FileNotFoundError(f"missing release binary: {source}")
        destination = staging / binary
        shutil.copy2(source, destination)
        if WINDOWS_TARGET_MARKER not in target:
            destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    marketplace = repo_root / ".agents" / "plugins" / "marketplace.json"
    plugin = repo_root / "plugins" / "gareji-progress"
    if not marketplace.is_file():
        raise FileNotFoundError(f"missing Marketplace manifest: {marketplace}")
    if not plugin.is_dir():
        raise FileNotFoundError(f"missing Gareji Progress Plugin: {plugin}")

    marketplace_destination = staging / ".agents" / "plugins"
    marketplace_destination.mkdir(parents=True)
    shutil.copy2(marketplace, marketplace_destination / "marketplace.json")
    shutil.copytree(
        plugin,
        staging / "plugins" / "gareji-progress",
        ignore=ignore_plugin_runtime,
    )

    for relative in (
        "README.md",
        "LICENSE",
        "NOTICE",
        "docs/setup-v0.md",
        "docs/platform-layout-v0.md",
    ):
        source = repo_root / relative
        if source.is_file():
            destination = staging / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)

    write_manifest(staging, version, target)
    return staging


def write_manifest(staging: Path, version: str, target: str) -> None:
    files = []
    for path in sorted(item for item in staging.rglob("*") if item.is_file()):
        files.append(
            {
                "path": path.relative_to(staging).as_posix(),
                "sha256": sha256(path),
                "size": path.stat().st_size,
            }
        )
    manifest = {
        "format": "gareji-distribution-v0",
        "target": target,
        "version": version,
        "files": files,
    }
    (staging / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def smoke_test(staging: Path, target: str) -> None:
    gareji = staging / executable_name("gareji", target)
    core = staging / executable_name("gareji-core", target)
    for executable in (gareji, core):
        result = subprocess.run(
            [str(executable), "--help"],
            cwd=staging,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"{executable.name} --help failed ({result.returncode}): {result.stderr}"
            )
    status_help = subprocess.run(
        [str(gareji), "status", "--help"],
        cwd=staging,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if status_help.returncode != 0:
        raise RuntimeError(
            f"gareji status --help failed ({status_help.returncode}): {status_help.stderr}"
        )

    with tempfile.TemporaryDirectory(prefix="gareji-smoke-") as temporary:
        profile = Path(temporary)
        workspace = profile / "sample-project"
        workspace.mkdir()
        context = workspace / "CONTEXT.md"
        context.write_text("# Distribution smoke test\n", encoding="utf-8")
        install_dir = profile / "install"
        command = [
            str(gareji),
            "--json",
            "setup",
            "--workspace",
            str(workspace.resolve()),
            "--context",
            str(context.resolve()),
            "--install-dir",
            str(install_dir.resolve()),
            "--codex-bin",
            str(Path(sys.executable).resolve()),
            "--dry-run",
        ]
        result = subprocess.run(
            command,
            cwd=staging,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"bundled setup dry-run failed ({result.returncode}): {result.stderr}"
            )
        try:
            report = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise RuntimeError("bundled setup did not return valid JSON") from error
        if not report.get("healthy") or len(report.get("steps", [])) < 4:
            raise RuntimeError(f"bundled setup returned an incomplete report: {report}")
        if install_dir.exists():
            raise RuntimeError("bundled setup dry-run wrote to the install directory")


def create_archive(staging: Path, target: str) -> Path:
    if WINDOWS_TARGET_MARKER in target:
        archive = staging.parent / f"{staging.name}.zip"
        if archive.exists():
            archive.unlink()
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for path in sorted(item for item in staging.rglob("*") if item.is_file()):
                output.write(path, (Path(staging.name) / path.relative_to(staging)).as_posix())
    else:
        archive = staging.with_name(f"{staging.name}.tar.gz")
        if archive.exists():
            archive.unlink()
        with tarfile.open(archive, "w:gz") as output:
            output.add(staging, arcname=staging.name)
    checksum = archive.with_name(f"{archive.name}.sha256")
    checksum.write_text(f"{sha256(archive)}  {archive.name}\n", encoding="utf-8")
    return archive


def package_distribution(
    repo_root: Path,
    release_dir: Path,
    output_dir: Path,
    target: str,
    run_smoke_test: bool = False,
) -> Path:
    staging = stage_distribution(repo_root, release_dir, output_dir, target)
    if run_smoke_test:
        smoke_test(staging, target)
    return create_archive(staging, target)


def main() -> int:
    args = parse_args()
    archive = package_distribution(
        args.repo_root,
        args.release_dir,
        args.output_dir,
        args.target,
        args.smoke_test,
    )
    print(archive.resolve())
    print(archive.with_name(f"{archive.name}.sha256").resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
