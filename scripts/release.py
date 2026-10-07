#!/usr/bin/env python3
"""Release configuration and preflight checks for AngelBot.

The normal development and test build intentionally excludes the updater plugin.
Production release jobs generate a small overlay config after CI injects the public
updater key; the private signing key is never written to disk by this script.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PACKAGE_JSON = ROOT / "package.json"
TAURI_CONFIG = ROOT / "src-tauri" / "tauri.conf.json"
CARGO_MANIFEST = ROOT / "src-tauri" / "Cargo.toml"
DEFAULT_CAPABILITY = ROOT / "src-tauri" / "capabilities" / "default.json"
DEFAULT_RELEASE_CONFIG = ROOT / "src-tauri" / "tauri.release.conf.json"
RELEASE_WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
UPDATER_ENDPOINT = (
    "https://github.com/shuimo233/AngelBot/releases/latest/download/latest.json"
)
SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
REVERSE_DOMAIN = re.compile(r"^[A-Za-z][A-Za-z0-9-]*(?:\.[A-Za-z0-9-]+){2,}$")


def load_json(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def load_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def normalized_permissions(capability: dict) -> set[str]:
    values: set[str] = set()
    for entry in capability.get("permissions", []):
        if isinstance(entry, str):
            values.add(entry)
        elif isinstance(entry, dict) and isinstance(entry.get("identifier"), str):
            values.add(entry["identifier"])
    return values


def check_release(tag: str | None) -> list[str]:
    errors: list[str] = []
    package = load_json(PACKAGE_JSON)
    tauri = load_json(TAURI_CONFIG)
    cargo = load_toml(CARGO_MANIFEST)
    capability = load_json(DEFAULT_CAPABILITY)

    versions = {
        "package.json": package.get("version"),
        "src-tauri/Cargo.toml": cargo.get("package", {}).get("version"),
        "src-tauri/tauri.conf.json": tauri.get("version"),
    }
    unique_versions = set(versions.values())
    if len(unique_versions) != 1:
        errors.append(
            "release versions differ: "
            + ", ".join(f"{path}={version!r}" for path, version in versions.items())
        )
    else:
        version = next(iter(unique_versions))
        if not isinstance(version, str) or not SEMVER.fullmatch(version):
            errors.append(f"release version is not valid SemVer: {version!r}")
        elif tag and tag != f"v{version}":
            errors.append(f"release tag {tag!r} must equal 'v{version}'")

    identifier = tauri.get("identifier")
    if not isinstance(identifier, str) or not REVERSE_DOMAIN.fullmatch(identifier):
        errors.append("Tauri identifier must be a stable reverse-domain identifier")

    bundle = tauri.get("bundle", {})
    if bundle.get("active") is not True:
        errors.append("Tauri bundle.active must be true")
    targets = bundle.get("targets")
    target_set = {targets} if isinstance(targets, str) else set(targets or [])
    if target_set != {"nsis"}:
        errors.append("Tauri bundle targets must contain only the per-user NSIS installer")
    for icon in bundle.get("icon", []):
        icon_path = ROOT / "src-tauri" / icon
        if not icon_path.is_file():
            errors.append(f"bundle icon is missing: {icon_path.relative_to(ROOT)}")

    capabilities = tauri.get("app", {}).get("security", {}).get("capabilities", [])
    if capabilities != ["default"]:
        errors.append("default Tauri config must expose only the default capability")

    permissions = normalized_permissions(capability)
    if "process:allow-restart" not in permissions:
        errors.append("default capability must allow controlled process restart")
    if any(value.startswith(("updater:", "wdio:")) for value in permissions):
        errors.append("updater and WDIO permissions must not exist in the default capability")

    dependencies = cargo.get("dependencies", {})
    updater = dependencies.get("tauri-plugin-updater", {})
    if not isinstance(updater, dict) or updater.get("optional") is not True:
        errors.append("tauri-plugin-updater must remain an optional dependency")
    production_feature = cargo.get("features", {}).get("production-updater", [])
    if "dep:tauri-plugin-updater" not in production_feature:
        errors.append("production-updater feature must enable the optional updater dependency")

    desktop_feature = cargo.get("features", {}).get("desktop-e2e", [])
    if not desktop_feature or not all(str(value).startswith("dep:tauri-plugin-wdio") for value in desktop_feature):
        errors.append("desktop-e2e must keep WDIO dependencies behind its feature flag")

    if not RELEASE_WORKFLOW.is_file():
        errors.append("GitHub release workflow is missing")
    else:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        required_release_fragments = (
            "tauri-apps/tauri-action@v0",
            "--features production-updater",
            "--config src-tauri/tauri.release.conf.json",
            "--bundles nsis",
            "TAURI_SIGNING_PRIVATE_KEY",
            "TAURI_UPDATER_PUBLIC_KEY",
        )
        for fragment in required_release_fragments:
            if fragment not in workflow:
                errors.append(f"release workflow is missing required configuration: {fragment}")

    return errors


def current_tag() -> str | None:
    result = subprocess.run(
        ["git", "describe", "--tags", "--exact-match", "HEAD"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip() if result.returncode == 0 else None


def write_updater_config(output: Path, public_key: str) -> None:
    public_key = public_key.strip()
    if not public_key:
        raise ValueError("TAURI_UPDATER_PUBLIC_KEY is empty")
    if "PRIVATE KEY" in public_key.upper():
        raise ValueError("expected a public updater key, received private-key material")

    config = {
        "bundle": {"createUpdaterArtifacts": True},
        "plugins": {
            "updater": {
                "pubkey": public_key,
                "endpoints": [UPDATER_ENDPOINT],
            }
        },
        "app": {
            "security": {
                "capabilities": [
                    "default",
                    {
                        "identifier": "production-updater",
                        "description": "Allow the main window to check and install signed AngelBot updates.",
                        "windows": ["main"],
                        "permissions": ["updater:default"],
                    },
                ]
            }
        },
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)

    check_parser = subcommands.add_parser("check", help="validate release invariants")
    check_parser.add_argument("--tag", help="require this release tag to match the project version")
    check_parser.add_argument(
        "--current-tag",
        action="store_true",
        help="validate the exact Git tag currently pointing at HEAD",
    )

    write_parser = subcommands.add_parser(
        "write-updater-config",
        help="write the CI-only signed-updater overlay config",
    )
    write_parser.add_argument("--output", type=Path, default=DEFAULT_RELEASE_CONFIG)
    args = parser.parse_args()

    if args.command == "check":
        tag = current_tag() if args.current_tag else args.tag
        if args.current_tag and tag is None:
            print("[FAIL] HEAD is not at an exact release tag", file=sys.stderr)
            return 1
        errors = check_release(tag)
        if errors:
            print("Release preflight failed:", file=sys.stderr)
            for error in errors:
                print(f"  - {error}", file=sys.stderr)
            return 1
        print("Release preflight passed.")
        return 0

    public_key = os.environ.get("TAURI_UPDATER_PUBLIC_KEY", "")
    try:
        write_updater_config(args.output, public_key)
    except ValueError as error:
        print(f"[FAIL] {error}", file=sys.stderr)
        return 1
    print(f"Wrote updater release config: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
