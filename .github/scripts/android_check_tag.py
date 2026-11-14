#!/usr/bin/env python3
"""Validate the Android release tag and expose version metadata to the workflow.

The Android release flow is tag-driven: pushing ``android-v<versionName>`` (for
example ``android-v1.3.4``) builds and publishes the APK. The tag must match the
repository root ``VERSION`` file exactly, otherwise the APK, GitHub Release,
update manifest and Pages download page would disagree about the version.

Outputs written to ``$GITHUB_OUTPUT`` (and printed for humans):

    version      1.3.4
    version_code 10304
    is_release   true|false
    tag          android-v1.3.4 (empty for branch pushes)

Usage:
    python3 .github/scripts/android_check_tag.py --ref "$GITHUB_REF"
    python3 .github/scripts/android_check_tag.py --ref refs/tags/android-v1.3.4
"""

from __future__ import annotations

import argparse
import os
import re
import sys

TAG_PREFIX = "android-v"
VERSION_PATTERN = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


def fail(message: str) -> "NoReturn":  # type: ignore[name-defined]
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def version_code(version: str) -> int:
    """Android versionCode derived from the release version: 1.3.4 -> 10304."""
    match = VERSION_PATTERN.match(version)
    if not match:
        fail(f"VERSION must be MAJOR.MINOR.PATCH, got {version!r}")
    major, minor, patch = (int(part) for part in match.groups())
    if patch > 99:
        fail(f"VERSION patch component must be 0..99 for the versionCode scheme, got {version}")
    return major * 10000 + minor * 100 + patch


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", required=True, help="Value of github.ref, e.g. refs/tags/android-v1.3.4")
    parser.add_argument("--version-file", default="VERSION", help="Repository VERSION file path")
    parser.add_argument(
        "--github-output",
        default=os.environ.get("GITHUB_OUTPUT", ""),
        help="File to append step outputs to (defaults to $GITHUB_OUTPUT)",
    )
    args = parser.parse_args()

    try:
        with open(args.version_file, encoding="utf-8") as handle:
            version = handle.read().strip()
    except FileNotFoundError:
        fail(f"version file not found: {args.version_file}")

    code = version_code(version)
    ref = args.ref.strip()
    is_release = ref.startswith("refs/tags/")
    tag = ""
    if is_release:
        tag = ref[len("refs/tags/") :]
        if not tag.startswith(TAG_PREFIX):
            fail(
                f"tag {tag!r} is not an Android release tag; expected "
                f"{TAG_PREFIX}<version> (e.g. {TAG_PREFIX}{version})"
            )
        tag_version = tag[len(TAG_PREFIX) :]
        if tag_version != version:
            fail(
                f"tag {tag!r} does not match repository VERSION {version!r}; "
                f"bump VERSION and commit it before tagging, or use tag "
                f"{TAG_PREFIX}{version}"
            )
        tag = f"{TAG_PREFIX}{tag_version}"

    outputs = {
        "version": version,
        "version_code": str(code),
        "is_release": "true" if is_release else "false",
        "tag": tag,
    }
    if args.github_output:
        with open(args.github_output, "a", encoding="utf-8") as handle:
            for key, value in outputs.items():
                handle.write(f"{key}={value}\n")

    print(
        f"version={version} version_code={code} "
        f"is_release={outputs['is_release']} tag={tag or '(none)'}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
