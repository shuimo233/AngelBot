#!/usr/bin/env python3
"""Canonical local and CI verification entry point for AngelBot.

Examples:
  python scripts/verify.py quick
  python scripts/verify.py full
  python scripts/verify.py backend
  python scripts/verify.py frontend
  python scripts/verify.py native-storage  # Windows only; synthetic OS-vault entries
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
TAURI_MANIFEST = "src-tauri/Cargo.toml"


@dataclass(frozen=True)
class Check:
    name: str
    command: tuple[str, ...]


CHECKS = {
    "workspace": (
        Check("diff hygiene", ("git", "diff", "--check")),
        Check("staged diff hygiene", ("git", "diff", "--cached", "--check")),
        Check("release preflight", (sys.executable, "scripts/release.py", "check")),
        Check("desktop build isolation", (sys.executable, "scripts/test_desktop_e2e.py")),
    ),
    "frontend": (
        Check("frontend unit tests", ("npm", "test", "--", "--run")),
        Check("frontend production build", ("npm", "run", "build")),
    ),
    "backend": (
        Check("Rust formatting", ("cargo", "fmt", "--manifest-path", TAURI_MANIFEST, "--check")),
        Check("Rust type check", ("cargo", "check", "--manifest-path", TAURI_MANIFEST)),
        Check(
            "Rust library tests",
            ("cargo", "test", "--manifest-path", TAURI_MANIFEST, "--lib", "--", "--test-threads=1"),
        ),
    ),
}

PROFILES = {
    "quick": CHECKS["workspace"]
    + CHECKS["frontend"][1:]
    + CHECKS["backend"][:2],
    "frontend": CHECKS["frontend"],
    "backend": CHECKS["backend"],
    "full": CHECKS["workspace"] + CHECKS["frontend"] + CHECKS["backend"],
    # Explicit opt-in: never append native credential I/O to backend/full/CI.
    "native-storage": (
        Check("native Windows storage credentials", (
            "cargo", "test", "--manifest-path", TAURI_MANIFEST, "--lib",
            "llm::chatgpt_auth::tests::native_profile_cutover_keeps_windows_vault_binding",
            "--", "--exact", "--ignored", "--nocapture", "--test-threads=1",
        )),
    ),
}


def isolated_environment() -> dict[str, str]:
    """Keep compiler/system configuration, never pass live model credentials."""
    sensitive = re.compile(r"TOKEN|API.?KEY|SECRET|PASSWORD|CREDENTIAL|(?:^|_)KEY(?:_|$)", re.IGNORECASE)
    env = {name: value for name, value in os.environ.items() if not sensitive.search(name)}
    env.update({"CARGO_NET_OFFLINE": "true", "NPM_CONFIG_OFFLINE": "true"})
    return env


def executable_available(command: str) -> bool:
    return shutil.which(command) is not None


def platform_command(command: tuple[str, ...]) -> tuple[str, ...]:
    """Return a command directly executable by the current platform."""
    resolved = shutil.which(command[0])
    if resolved is None:
        return command
    if os.name == "nt" and Path(resolved).suffix.lower() in {".cmd", ".bat"}:
        return (
            os.environ.get("COMSPEC", "cmd.exe"),
            "/d",
            "/s",
            "/c",
            subprocess.list2cmdline(command),
        )
    return (resolved, *command[1:])


def run_check(check: Check, fail_fast: bool) -> bool:
    executable = check.command[0]
    if not executable_available(executable):
        print(f"\n[FAIL] {check.name}: required executable '{executable}' was not found.")
        return False

    print(f"\n==> {check.name}\n    {' '.join(check.command)}")
    started = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(prefix="angelbot-verify-config-") as config_root:
            env = {key: value for key, value in isolated_environment().items()
                   if not key.startswith(("ANGELBOT_", "VITE_ANGELBOT_"))}
            # The Rust override is cfg(test) only: normal builds ignore it.
            # Its lifecycle belongs to this parent, including failed checks.
            env["ANGELBOT_PRIVATE_VERIFY_CONFIG_DIR"] = config_root
            result = subprocess.run(platform_command(check.command), cwd=ROOT, check=False, env=env)
    except OSError as error:
        print(f"<== {check.name}: FAIL (could not execute or clean up: {error})")
        return False
    elapsed = time.monotonic() - started
    outcome = "PASS" if result.returncode == 0 else f"FAIL ({result.returncode})"
    print(f"<== {check.name}: {outcome} ({elapsed:.1f}s)")
    return result.returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", choices=PROFILES, nargs="?", default="quick")
    parser.add_argument("--fail-fast", action="store_true", help="stop after the first failed check")
    args = parser.parse_args()
    if args.profile == "native-storage" and os.name != "nt":
        parser.error("native-storage requires Windows; it must not pass with zero native tests")

    checks = PROFILES[args.profile]
    print(f"AngelBot verification profile: {args.profile} ({len(checks)} checks)")
    failures: list[str] = []
    for check in checks:
        if not run_check(check, args.fail_fast):
            failures.append(check.name)
            if args.fail_fast:
                break

    if failures:
        print("\nVerification failed:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print("\nVerification passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
