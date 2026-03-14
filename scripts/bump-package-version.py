#!/usr/bin/env python3
"""
Update [package].version in a Cargo.toml while preserving the rest of the file.

Usage:
    python3 scripts/bump-package-version.py path/to/Cargo.toml 0.4.0
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SEMVER_RE = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def split_package_section(content: str) -> tuple[int, int]:
    start_match = re.search(r"(?m)^\[package\]\s*$", content)
    if not start_match:
        fail("missing [package] section")

    next_section_match = re.search(r"(?m)^\[.+\]\s*$", content[start_match.end() :])
    start = start_match.start()
    end = (
        start_match.end() + next_section_match.start()
        if next_section_match
        else len(content)
    )
    return start, end


def bump_manifest_version(manifest_path: Path, new_version: str) -> None:
    if not SEMVER_RE.match(new_version):
        fail(f"invalid semantic version: {new_version}")

    content = manifest_path.read_text(encoding="utf-8")
    package_start, package_end = split_package_section(content)
    package_section = content[package_start:package_end]

    replaced, count = re.subn(
        r'(?m)^version\s*=\s*"[^"]+"\s*$',
        f'version = "{new_version}"',
        package_section,
        count=1,
    )
    if count != 1:
        fail("missing version field in [package] section")

    manifest_path.write_text(
        content[:package_start] + replaced + content[package_end:],
        encoding="utf-8",
    )


def main(argv: list[str]) -> None:
    if len(argv) != 3:
        fail("usage: python3 scripts/bump-package-version.py path/to/Cargo.toml 0.4.0")

    manifest_path = Path(argv[1])
    if not manifest_path.exists():
        fail(f"manifest not found: {manifest_path}")

    bump_manifest_version(manifest_path, argv[2])


if __name__ == "__main__":
    main(sys.argv)
