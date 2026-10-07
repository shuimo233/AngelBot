#!/usr/bin/env python3
"""Opt-in Windows UIA smoke through AngelBot's real confirmation/adapter path.

Run on an interactive Windows desktop: python scripts/native_uia_smoke.py
This is intentionally not part of scripts/verify.py full's offline profile.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
TEST_NAME = "commands::message::tests::native_uia_fixture_confirmation"
CONFIRMED_TEXT = "Native UIA confirmed replacement — 中文"


def isolated_environment() -> dict[str, str]:
    """Keep compiler/system configuration, never pass live model credentials."""
    sensitive = re.compile(r"TOKEN|API.?KEY|SECRET|PASSWORD|CREDENTIAL", re.IGNORECASE)
    env = {name: value for name, value in os.environ.items() if not sensitive.search(name)}
    env.update({"CARGO_NET_OFFLINE": "true", "NPM_CONFIG_OFFLINE": "true"})
    return env


def build_test_binary() -> Path:
    command = [
        "cargo", "test", "--offline", "--manifest-path", "src-tauri/Cargo.toml",
        "--lib", "--no-run", "--message-format=json",
    ]
    print("Building the opt-in Rust smoke (offline dependencies only)...", flush=True)
    build = subprocess.run(
        command, cwd=ROOT, env=isolated_environment(),
        capture_output=True, text=True, encoding="utf-8",
    )
    candidates: list[Path] = []
    for line in build.stdout.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("reason") == "compiler-message":
            rendered = event.get("message", {}).get("rendered")
            if rendered:
                print(rendered, end="", file=sys.stderr)
        if (
            event.get("reason") == "compiler-artifact"
            and event.get("target", {}).get("name") == "angelbot"
            and "lib" in event.get("target", {}).get("kind", [])
            and event.get("profile", {}).get("test")
            and event.get("executable")
        ):
            candidates.append(Path(event["executable"]))
    if build.returncode:
        print(build.stderr, file=sys.stderr)
        raise RuntimeError("Rust smoke build failed; provision cached dependencies before retrying")
    if len(candidates) != 1:
        raise RuntimeError(f"Expected one AngelBot library test binary, found {len(candidates)}")
    return candidates[0]


def build_fixture(directory: Path) -> Path:
    framework = Path(os.environ["WINDIR"]) / "Microsoft.NET" / "Framework64" / "v4.0.30319"
    compiler = framework / "csc.exe"
    if not compiler.is_file():
        framework = Path(os.environ["WINDIR"]) / "Microsoft.NET" / "Framework" / "v4.0.30319"
        compiler = framework / "csc.exe"
    if not compiler.is_file():
        raise RuntimeError("The Windows .NET Framework C# compiler is required for the owned WPF fixture")
    executable = directory / "AngelBotNativeUiaFixture.exe"
    subprocess.run(
        [
            str(compiler), "/nologo", "/target:winexe", f"/out:{executable}",
            f"/lib:{framework / 'WPF'}", "/reference:WindowsBase.dll",
            "/reference:PresentationCore.dll", "/reference:PresentationFramework.dll",
            "/reference:System.Xaml.dll", "/reference:System.Web.Extensions.dll",
            str(ROOT / "scripts" / "fixtures" / "NativeUiaFixture.cs"),
        ],
        cwd=ROOT, env=isolated_environment(), check=True,
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    return executable


def read_snapshot(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError):
        return None


def await_snapshot(path: Path, fixture: subprocess.Popen, expected_text: str, timeout: float) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if fixture.poll() is not None:
            raise RuntimeError(f"Owned fixture exited early (code {fixture.returncode}); interactive desktop required")
        snapshot = read_snapshot(path)
        if snapshot and snapshot.get("ordinaryValue") == expected_text:
            if snapshot.get("processId") != fixture.pid:
                raise AssertionError("Fixture readiness belongs to an unexpected process")
            return snapshot
        time.sleep(0.05)
    raise RuntimeError(f"Owned fixture did not report expected text within {timeout:g}s")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, help="Use an already-built AngelBot library test binary")
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("This opt-in test requires an interactive Windows desktop")

    test_binary = args.test_binary.resolve() if args.test_binary else build_test_binary()
    listed = subprocess.run(
        [str(test_binary), "--list", "--format", "terse"], cwd=ROOT,
        env=isolated_environment(), check=True, capture_output=True, text=True, encoding="utf-8",
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    if f"{TEST_NAME}: test" not in listed.stdout.splitlines():
        raise RuntimeError("Native UIA smoke test is missing from this binary; rebuild the current sources")

    with tempfile.TemporaryDirectory(prefix="angelbot-native-uia-") as runtime:
        directory = Path(runtime).resolve()
        executable = build_fixture(directory)
        snapshot_path = directory / "fixture-state.json"
        window_title = f"AngelBot Native UIA Fixture {uuid.uuid4().hex}"
        print("Starting owned WPF fixture (a temporary non-activating window will be visible)...", flush=True)
        fixture = subprocess.Popen(
            [str(executable), str(snapshot_path), window_title], cwd=directory,
            env=isolated_environment(),
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )
        try:
            initial = await_snapshot(snapshot_path, fixture, "ordinary-seed", 15)
            if initial.get("windowTitle") != window_title:
                raise AssertionError("Unexpected owned fixture window title")
            env = isolated_environment()
            env.update({
                "ANGELBOT_NATIVE_UIA_SMOKE_EXE": str(executable),
                "ANGELBOT_NATIVE_UIA_SMOKE_PID": str(fixture.pid),
                "ANGELBOT_NATIVE_UIA_SMOKE_TITLE": window_title,
                "ANGELBOT_NATIVE_UIA_SMOKE_SNAPSHOT": str(snapshot_path),
                "ANGELBOT_NATIVE_UIA_SMOKE_TEXT": CONFIRMED_TEXT,
            })
            test = subprocess.run(
                [str(test_binary), "--exact", TEST_NAME, "--ignored", "--nocapture", "--test-threads=1"],
                cwd=ROOT, env=env, timeout=120,
                capture_output=True, text=True, encoding="utf-8", errors="replace",
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            print(test.stdout, end="", flush=True)
            if test.stderr:
                print(test.stderr, end="", file=sys.stderr, flush=True)
            if test.returncode:
                # Fixture-only diagnostics: no UIA values or user-app contents.
                snapshot = read_snapshot(snapshot_path)
                if snapshot and snapshot.get("processId") == fixture.pid:
                    keys = (
                        "ordinaryReadOnly", "invocationCount", "selected",
                        "menuExpanded", "selectionCount", "expansionCount", "collapseCount",
                        "scrollOffset", "scrollableHeight", "scrollViewportHeight",
                        "scrollHorizontalOffset", "scrollCount", "scrollAtTop", "scrollAtBottom",
                    )
                    print(
                        "Owned fixture state at failure: "
                        + json.dumps({key: snapshot.get(key) for key in keys}),
                        file=sys.stderr, flush=True,
                    )
            test.check_returncode()
            final = await_snapshot(snapshot_path, fixture, CONFIRMED_TEXT, 5)
            expected_unchanged = {
                "readOnlyValue": "read-only-seed",
                "duplicateOneValue": "duplicate-one-seed",
                "duplicateTwoValue": "duplicate-two-seed",
                "passwordUnchanged": True,
            }
            for key, expected in expected_unchanged.items():
                if final.get(key) != expected:
                    raise AssertionError(f"Safety fixture changed: {key}")
            if final.get("invocationCount") != 1:
                raise AssertionError("Owned fixture button must be invoked exactly once")
            expected_control_state = {"selected": True, "menuExpanded": False, "selectionCount": 1, "expansionCount": 1, "collapseCount": 1}
            for key, expected in expected_control_state.items():
                if final.get(key) != expected:
                    raise AssertionError(f"Owned fixture control-state receipt mismatch: {key}")
            if (
                final.get("scrollCount") != 2
                or final.get("scrollOffset") != 0
                or final.get("scrollHorizontalOffset") != 0
                or final.get("scrollAtTop") is not True
                or not isinstance(final.get("scrollableHeight"), (int, float))
                or final["scrollableHeight"] <= 0
            ):
                raise AssertionError("Owned scroll area must move down/up once each, preserve horizontal position and finish at top")
            print("PASS: real observe -> pending confirmation -> preflight -> confirmed SetValue/Invoke/Select/Expand/Collapse/ScrollDown/ScrollUp -> independent readback, edge no-op and re-observation.")
        finally:
            # Only the exact child handle is terminated; never kill by name or scan user apps.
            if fixture.poll() is None:
                fixture.terminate()
                try:
                    fixture.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    fixture.kill()
                    fixture.wait(timeout=5)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Native UIA smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
