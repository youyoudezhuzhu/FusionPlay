#!/usr/bin/env python3
"""Assemble and verify the GitHub Pages artifact for an Android update.

The workflow downloads the release APK and ``update-manifest.json`` from a
GitHub Release, then calls this script. It verifies the manifest against the
actual APK bytes and lays out the Pages artifact exactly as the app and the
download page expect:

    _site/index.html                              (docs/update/index.html)
    _site/update/android/stable.json              (manifest copy)
    _site/update/android/dl/latest.apk            (verified release APK)
    _site/update/android/SHA256SUMS-<version>.txt

It refuses to stage an APK whose bytes do not match the manifest, so a
corrupted/partial download can never be served as ``latest.apk``.

Example:
    python3 .github/scripts/android_prepare_pages.py \
        --manifest /tmp/rel/update-manifest.json \
        --apk /tmp/rel/FusionPlay-Android-1.3.4.apk \
        --index docs/update/index.html \
        --output-dir _site \
        --tag android-v1.3.4 \
        --meta-env /tmp/rel/meta.env
"""

from __future__ import annotations

import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import sys

VERSION_PATTERN = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
PUBLISHED_AT_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
REQUIRED_FIELDS = (
    "schema_version",
    "channel",
    "published",
    "version_name",
    "version_code",
    "sha256",
    "signer_sha256",
    "apk_url",
    "apk_url_alt",
    "apk_size",
    "source_commit",
    "notes",
    "published_at",
)


def fail(message: str) -> "NoReturn":  # type: ignore[name-defined]
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def sha256_of(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
    except FileNotFoundError:
        fail(f"manifest not found: {path}")
    except json.JSONDecodeError as error:
        fail(f"manifest is not valid JSON ({path}): {error}")
    if not isinstance(data, dict):
        fail(f"manifest must be a JSON object: {path}")
    return data


def validate_manifest(manifest: dict, apk_path: str, tag: str) -> str:
    missing = [field for field in REQUIRED_FIELDS if field not in manifest]
    if missing:
        fail(f"manifest is missing required fields: {', '.join(missing)}")
    extra = sorted(set(manifest) - set(REQUIRED_FIELDS))
    if extra:
        print(f"warning: manifest has extra fields, keeping them: {', '.join(extra)}", file=sys.stderr)

    if manifest["schema_version"] != 1:
        fail(f"unsupported manifest schema_version {manifest['schema_version']!r} (expected 1)")
    if manifest["channel"] != "stable":
        fail(f"manifest channel must be 'stable', got {manifest['channel']!r}")
    if not isinstance(manifest["published"], bool):
        fail("manifest published must be a boolean")

    version = str(manifest["version_name"])
    match = VERSION_PATTERN.match(version)
    if not match:
        fail(f"manifest version_name must be MAJOR.MINOR.PATCH, got {version!r}")
    major, minor, patch = (int(part) for part in match.groups())
    expected_code = major * 10000 + minor * 100 + patch
    code = manifest["version_code"]
    if not isinstance(code, int) or code != expected_code:
        fail(f"manifest version_code {code!r} does not match version {version} (expected {expected_code})")

    apk_name = os.path.basename(apk_path)
    expected_apk_name = f"FusionPlay-Android-{version}.apk"
    if apk_name != expected_apk_name:
        fail(f"APK asset name {apk_name!r} does not match expected {expected_apk_name!r}")

    for field in ("sha256", "signer_sha256"):
        value = str(manifest[field])
        if not SHA256_PATTERN.match(value):
            fail(f"manifest {field} is not a lowercase 64 character hex SHA-256: {value!r}")

    actual_sha = sha256_of(apk_path)
    if actual_sha != manifest["sha256"]:
        fail(
            "APK SHA-256 does not match the manifest: "
            f"file={actual_sha} manifest={manifest['sha256']}"
        )
    actual_size = os.path.getsize(apk_path)
    if actual_size != manifest["apk_size"]:
        fail(f"APK size does not match the manifest: file={actual_size} manifest={manifest['apk_size']}")

    apk_url = str(manifest["apk_url"])
    expected_url_path = "/update/android/dl/latest.apk"
    if expected_url_path not in apk_url:
        fail(f"manifest apk_url must point at {expected_url_path}, got {apk_url!r}")
    alt_url = str(manifest["apk_url_alt"])
    if f"/releases/download/{tag}/" not in alt_url or not alt_url.endswith(expected_apk_name):
        fail(
            "manifest apk_url_alt must point at the GitHub Release asset "
            f"releases/download/{tag}/{expected_apk_name}, got {alt_url!r}"
        )

    if not PUBLISHED_AT_PATTERN.match(str(manifest["published_at"])):
        fail(f"manifest published_at must look like 2026-11-15T00:00:00Z, got {manifest['published_at']!r}")

    if str(manifest["source_commit"]) and not re.match(r"^[0-9a-f]{7,40}$", str(manifest["source_commit"])):
        fail(f"manifest source_commit must be a hex commit id, got {manifest['source_commit']!r}")

    return apk_name


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, help="update-manifest.json downloaded from the release")
    parser.add_argument("--apk", required=True, help="Release APK downloaded from the release")
    parser.add_argument("--index", required=True, help="Download page source (docs/update/index.html)")
    parser.add_argument("--output-dir", required=True, help="Pages artifact directory to create (e.g. _site)")
    parser.add_argument("--tag", default="", help="Release tag, e.g. android-v1.3.4")
    parser.add_argument("--meta-env", default="", help="Optional KEY=VALUE file for later workflow steps")
    args = parser.parse_args()

    if not os.path.isfile(args.apk):
        fail(f"APK not found: {args.apk}")
    if not os.path.isfile(args.index):
        fail(f"download page not found: {args.index}")

    manifest = load_manifest(args.manifest)
    apk_name = validate_manifest(manifest, args.apk, args.tag.strip())
    version = str(manifest["version_name"])

    with open(args.index, encoding="utf-8") as handle:
        index_html = handle.read()
    # Keep the static page and the artifact layout in sync: the page fetches the
    # manifest with this relative URL and links this APK file.
    for expected in ("update/android/stable.json", "update/android/dl/latest.apk"):
        if expected not in index_html:
            fail(f"download page {args.index} does not reference {expected!r}")

    output_dir = os.path.abspath(args.output_dir)
    android_dir = os.path.join(output_dir, "update", "android")
    download_dir = os.path.join(android_dir, "dl")
    os.makedirs(download_dir, exist_ok=True)

    # Remove stale files from earlier runs so only the current version is served.
    for stale in glob.glob(os.path.join(android_dir, "SHA256SUMS-*.txt")):
        os.remove(stale)

    shutil.copyfile(args.index, os.path.join(output_dir, "index.html"))

    with open(os.path.join(android_dir, "stable.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, ensure_ascii=False, indent=2)
        handle.write("\n")

    apk_destination = os.path.join(download_dir, "latest.apk")
    shutil.copyfile(args.apk, apk_destination)
    if sha256_of(apk_destination) != manifest["sha256"]:
        fail("staged latest.apk does not match the manifest SHA-256")

    sums_path = os.path.join(android_dir, f"SHA256SUMS-{version}.txt")
    with open(sums_path, "w", encoding="utf-8") as handle:
        handle.write(f"{manifest['sha256']}  {apk_name}\n")

    if args.meta_env:
        meta = {
            "VERSION": version,
            "VERSION_CODE": str(manifest["version_code"]),
            "TAG": args.tag.strip(),
            "SHA256": str(manifest["sha256"]),
            "SIGNER_SHA256": str(manifest["signer_sha256"]),
            "SOURCE_COMMIT": str(manifest["source_commit"]),
            "PUBLISHED_AT": str(manifest["published_at"]),
            "APK_NAME": apk_name,
            "APK_SIZE": str(manifest["apk_size"]),
        }
        with open(args.meta_env, "w", encoding="utf-8") as handle:
            for key, value in meta.items():
                handle.write(f"{key}={value}\n")

    staged = [
        os.path.join(output_dir, "index.html"),
        os.path.join(android_dir, "stable.json"),
        apk_destination,
        sums_path,
    ]
    print(f"Pages artifact staged at {output_dir}:")
    for path in staged:
        print(f"  {os.path.relpath(path, output_dir)}  ({os.path.getsize(path)} bytes)")
    print(f"serving {version} (versionCode {manifest['version_code']}) from {manifest['apk_url']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
