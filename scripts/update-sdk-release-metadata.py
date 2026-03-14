#!/usr/bin/env python3
"""
Update codex-app-server-sdk release metadata in the manifest and README status lines.

Usage:
    python3 scripts/update-sdk-release-metadata.py 0.4.0
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGET_FILES = [
    ROOT / "README.md",
    ROOT / "crates" / "sdk" / "README.md",
]
STATUS_LINE = re.compile(r'(?m)^- `\d+\.\d+\.\d+(?:[-+][^`]*)?`$')
SEMVER_RE = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def replace_status_version(path: Path, version: str) -> None:
    content = path.read_text(encoding="utf-8")
    replaced, count = STATUS_LINE.subn(f"- `{version}`", content, count=1)
    if count != 1:
        fail(f"expected one status version line in {path}")
    path.write_text(replaced, encoding="utf-8")


def main(argv: list[str]) -> None:
    if len(argv) != 2:
        fail("usage: python3 scripts/update-sdk-release-metadata.py 0.4.0")

    version = argv[1]
    if not SEMVER_RE.match(version):
        fail(f"invalid semantic version: {version}")

    subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts" / "bump-package-version.py"),
            str(ROOT / "crates" / "sdk" / "Cargo.toml"),
            version,
        ],
        check=True,
    )

    for path in TARGET_FILES:
        replace_status_version(path, version)


if __name__ == "__main__":
    main(sys.argv)
