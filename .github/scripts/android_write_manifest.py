#!/usr/bin/env python3
"""Generate the Android update manifest (schema 1) for a release APK.

The manifest is uploaded as a GitHub Release asset and later copied to
``updates/android/stable.json`` (raw fallback) and to the Pages site
``/FusionPlay/update/android/stable.json`` by ``android-pages.yml``.

Example:
    python3 .github/scripts/android_write_manifest.py \
        --version 1.3.4 --apk artifacts/releases/FusionPlay-Android-1.3.4.apk \
        --signer-sha256 4836fb96c2ef76fd81ac385455f1b0154af77fadae1b45bbb752b192499dc12f \
        --source-commit "$GITHUB_SHA" \
        --notes-file docs/update/android/notes.md \
        --tag android-v1.3.4 \
        --output artifacts/releases/update-manifest.json

Fields (schema 1) are exactly the contract consumed by the Android app:
    schema_version, channel, published, version_name, version_code, sha256,
    signer_sha256, apk_url, apk_url_alt, apk_size, source_commit, notes,
    published_at
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import re
import sys

VERSION_PATTERN = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
COMMIT_PATTERN = re.compile(r"^[0-9a-f]{7,40}$")
PUBLISHED_AT_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
REPOSITORY_PATTERN = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")

DEFAULT_REPOSITORY = "youyoudezhuzhu/FusionPlay"
DEFAULT_PAGES_BASE = "https://youyoudezhuzhu.github.io/FusionPlay"


def fail(message: str) -> "NoReturn":  # type: ignore[name-defined]
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def normalize_sha256(value: str, label: str) -> str:
    normalized = value.strip().replace(":", "").replace(" ", "").lower()
    if not SHA256_PATTERN.match(normalized):
        fail(f"{label} must be a 64 character hex SHA-256, got {value!r}")
    return normalized


def sha256_of(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def version_code(version: str) -> int:
    match = VERSION_PATTERN.match(version)
    if not match:
        fail(f"--version must be MAJOR.MINOR.PATCH, got {version!r}")
    major, minor, patch = (int(part) for part in match.groups())
    if patch > 99:
        fail(f"--version patch component must be 0..99 for the versionCode scheme, got {version}")
    return major * 10000 + minor * 100 + patch


def utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )


def read_notes(path: str) -> str:
    if not path:
        return ""
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read().strip("\n")
    except FileNotFoundError:
        fail(f"notes file not found: {path}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Version name, e.g. 1.3.4")
    parser.add_argument(
        "--version-code",
        type=int,
        default=None,
        help="Expected Android versionCode; cross-checked against --version when provided",
    )
    parser.add_argument("--apk", required=True, help="Path to the release APK")
    parser.add_argument("--signer-sha256", required=True, help="APK signing certificate SHA-256")
    parser.add_argument("--source-commit", required=True, help="Git commit the APK was built from")
    parser.add_argument("--notes-file", default="", help="Markdown release notes file")
    parser.add_argument("--notes", default=None, help="Inline notes (overrides --notes-file)")
    parser.add_argument("--tag", default="", help="Release tag (defaults to android-v<version>)")
    parser.add_argument("--repository", default=DEFAULT_REPOSITORY, help="owner/repo for the release mirror URL")
    parser.add_argument("--pages-base", default=DEFAULT_PAGES_BASE, help="GitHub Pages base URL")
    parser.add_argument(
        "--published-at",
        default="",
        help="UTC timestamp (YYYY-MM-DDTHH:MM:SSZ); defaults to now",
    )
    parser.add_argument("--output", required=True, help="Manifest JSON output path")
    parser.add_argument("--meta-env", default="", help="Optional KEY=VALUE output for later workflow steps")
    args = parser.parse_args()

    if not os.path.isfile(args.apk):
        fail(f"APK not found: {args.apk}")
    if not REPOSITORY_PATTERN.match(args.repository):
        fail(f"--repository must be owner/repo, got {args.repository!r}")

    code = version_code(args.version)
    if args.version_code is not None and args.version_code != code:
        fail(
            f"--version-code {args.version_code} does not match "
            f"--version {args.version} (expected {code})"
        )
    signer = normalize_sha256(args.signer_sha256, "--signer-sha256")
    source_commit = args.source_commit.strip().lower()
    if not COMMIT_PATTERN.match(source_commit):
        fail(f"--source-commit must be a 7..40 character hex commit id, got {args.source_commit!r}")

    published_at = args.published_at.strip() or utc_now()
    if not PUBLISHED_AT_PATTERN.match(published_at):
        fail(f"--published-at must look like 2026-11-15T00:00:00Z, got {published_at!r}")

    tag = args.tag.strip() or f"android-v{args.version}"
    if not tag.startswith("android-v"):
        fail(f"--tag must start with android-v, got {tag!r}")

    apk_sha256 = sha256_of(args.apk)
    apk_size = os.path.getsize(args.apk)
    apk_name = f"FusionPlay-Android-{args.version}.apk"
    pages_base = args.pages_base.rstrip("/")

    notes = args.notes if args.notes is not None else read_notes(args.notes_file)

    manifest = {
        "schema_version": 1,
        "channel": "stable",
        "published": True,
        "version_name": args.version,
        "version_code": code,
        "sha256": apk_sha256,
        "signer_sha256": signer,
        "apk_url": f"{pages_base}/update/android/dl/latest.apk?v={code}",
        "apk_url_alt": f"https://github.com/{args.repository}/releases/download/{tag}/{apk_name}",
        "apk_size": apk_size,
        "source_commit": source_commit,
        "notes": notes,
        "published_at": published_at,
    }

    output_directory = os.path.dirname(os.path.abspath(args.output))
    if output_directory:
        os.makedirs(output_directory, exist_ok=True)
    with open(args.output, "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, ensure_ascii=False, indent=2)
        handle.write("\n")

    if args.meta_env:
        meta = {
            "VERSION": args.version,
            "VERSION_CODE": str(code),
            "TAG": tag,
            "SHA256": apk_sha256,
            "SIGNER_SHA256": signer,
            "SOURCE_COMMIT": source_commit,
            "PUBLISHED_AT": published_at,
            "APK_NAME": apk_name,
            "APK_SIZE": str(apk_size),
        }
        with open(args.meta_env, "w", encoding="utf-8") as handle:
            for key, value in meta.items():
                handle.write(f"{key}={value}\n")

    print(
        f"wrote {args.output}: {args.version} (code {code}), "
        f"{apk_size} bytes, sha256 {apk_sha256}"
    )
    print(f"apk_url={manifest['apk_url']}")
    print(f"apk_url_alt={manifest['apk_url_alt']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
