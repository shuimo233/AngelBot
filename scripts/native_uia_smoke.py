#!/usr/bin/env python3
"""Opt-in Windows UIA smoke through AngelBot's real confirmation/adapter path.

Run on an interactive Windows desktop: python scripts/native_uia_smoke.py
This is intentionally not part of scripts/verify.py full's offline profile.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from typing import Iterator, NamedTuple
import uuid

from verify import isolated_environment

ROOT = Path(__file__).resolve().parents[1]
TEST_NAME = "commands::message::tests::native_uia_fixture_confirmation"
CAPTURE_TEST_NAME = "commands::message::tests::native_uia_fixture_capture"
VISUAL_CLICK_TEST_NAME = "commands::message::tests::native_visual_click_fixture_confirmation"
CONFIRMED_TEXT = "Native UIA confirmed replacement — 中文"


def build_cargo_binary(*, library_test: bool) -> Path:
    command = ["cargo", "test" if library_test else "build", "--offline", "--manifest-path", "src-tauri/Cargo.toml"]
    command.extend(["--lib", "--no-run"] if library_test else ["--bin", "angelbot"])
    command.append("--message-format=json")
    label = "library test" if library_test else "isolated capture-helper"
    print(f"Building the current AngelBot {label} binary (offline dependencies only)...", flush=True)
    env = isolated_environment()
    if not library_test:
        # A helper build must never replace the user's ordinary debug binary.
        env["CARGO_TARGET_DIR"] = str(ROOT / "src-tauri/target/native-capture-helper")
    build = subprocess.run(
        command, cwd=ROOT, env=env,
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
            and ("lib" if library_test else "bin") in event.get("target", {}).get("kind", [])
            and bool(event.get("profile", {}).get("test")) == library_test
            and event.get("executable")
        ):
            candidates.append(Path(event["executable"]))
    if build.returncode:
        print(build.stderr, file=sys.stderr)
        raise RuntimeError(f"AngelBot {label} build failed; provision cached dependencies before retrying")
    if len(candidates) != 1:
        raise RuntimeError(f"Expected one AngelBot {label} binary, found {len(candidates)}")
    return candidates[0].resolve()


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


class OwnedFixture(NamedTuple):
    executable: Path
    snapshot_path: Path
    title: str
    process: subprocess.Popen


@contextlib.contextmanager
def owned_fixture(directory: Path, env: dict[str, str], *, dense: bool = False) -> Iterator[OwnedFixture]:
    """One owned WPF child; callers own its directory and build-before-start order."""
    directory = directory.resolve()
    executable = build_fixture(directory)
    snapshot_path = directory / "fixture-state.json"
    title = f"AngelBot Native UIA Fixture {uuid.uuid4().hex}"
    print("Starting owned WPF fixture (a temporary non-activating window will be visible)...", flush=True)
    process = subprocess.Popen(
        [str(executable), str(snapshot_path), title], cwd=directory, env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    try:
        initial = await_snapshot(snapshot_path, process, "ordinary-seed", 15)
        if initial.get("windowTitle") != title:
            raise AssertionError("Unexpected owned fixture window title")
        if dense:
            snapshot_path.with_suffix(".command").write_text("dense", encoding="utf-8")
            deadline = time.monotonic() + 5
            while True:
                if process.poll() is not None:
                    raise RuntimeError("Owned fixture exited before its dense panel was ready")
                snapshot = read_snapshot(snapshot_path)
                if snapshot:
                    if snapshot.get("processId") != process.pid or snapshot.get("windowTitle") != title:
                        raise AssertionError("Dense fixture readiness belongs to an unexpected window")
                    if snapshot.get("denseMode") is True:
                        break
                if time.monotonic() >= deadline:
                    raise RuntimeError("Owned fixture dense panel was not ready within 5s")
                time.sleep(0.05)
        yield OwnedFixture(executable, snapshot_path, title, process)
    finally:
        # Only the exact child handle is terminated; never kill by name or scan user apps.
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, help="Use an already-built AngelBot library test binary")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--capture-only", action="store_true", help="Run only the existing single-window capture checks")
    modes.add_argument("--controls-only", action="store_true",
                       help="Run only UIA confirmation/control checks; never build or replace the normal capture-helper binary")
    modes.add_argument("--visual-click", action="store_true",
                       help="Opt-in one click in the owned pixel canary; requires foreground time and a user-activated fixture window")
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("This opt-in test requires an interactive Windows desktop")

    test_binary = args.test_binary.resolve() if args.test_binary else build_cargo_binary(library_test=True)
    listed = subprocess.run(
        [str(test_binary), "--list", "--format", "terse"], cwd=ROOT,
        env=isolated_environment(), check=True, capture_output=True, text=True, encoding="utf-8",
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    test_names = ([VISUAL_CLICK_TEST_NAME] if args.visual_click else [CAPTURE_TEST_NAME] if args.capture_only else [TEST_NAME]
                  if args.controls_only else [TEST_NAME, CAPTURE_TEST_NAME])
    if any(f"{name}: test" not in listed.stdout.splitlines() for name in test_names):
        raise RuntimeError("Native UIA smoke tests are missing from this binary; rebuild the current sources")
    # A libtest executable cannot enter the production helper's main function.
    # Build an isolated capture helper even when reusing the library test binary;
    # never replace the user's normal debug executable.
    capture_binary = (build_cargo_binary(library_test=False)
                      if CAPTURE_TEST_NAME in test_names or args.visual_click else None)

    with tempfile.TemporaryDirectory(prefix="angelbot-native-uia-") as runtime:
        directory = Path(runtime).resolve()
        with owned_fixture(directory, isolated_environment(), dense=args.capture_only or args.visual_click) as owned:
            executable, snapshot_path, window_title, fixture = owned
            env = isolated_environment()
            env.update({
                "ANGELBOT_NATIVE_UIA_SMOKE_EXE": str(executable),
                "ANGELBOT_NATIVE_UIA_SMOKE_PID": str(fixture.pid),
                "ANGELBOT_NATIVE_UIA_SMOKE_TITLE": window_title,
                "ANGELBOT_NATIVE_UIA_SMOKE_SNAPSHOT": str(snapshot_path),
                "ANGELBOT_NATIVE_UIA_SMOKE_TEXT": CONFIRMED_TEXT,
            })
            if capture_binary is not None:
                env["ANGELBOT_NATIVE_UIA_CAPTURE_EXE"] = str(capture_binary)
            for test_name in test_names:
                test = subprocess.run(
                    [str(test_binary), "--exact", test_name, "--ignored", "--nocapture", "--test-threads=1"],
                    # The control journey now also opens/closes real extra
                    # HWNDs and proves the ambiguous-window refusal. Individual
                    # production operations retain their existing deadlines.
                    cwd=ROOT, env=env, timeout=30 if args.capture_only else 150,
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
                            "mouseMoveCount", "mouseDownCount", "mouseWheelCount", "keyDownCount", "auxiliaryOpen", "secondWindowOpen",
                            "ordinaryReadOnly", "invocationCount", "selected",
                            "menuExpanded", "selectionCount", "expansionCount", "collapseCount",
                            "checkboxChecked", "checkboxState", "checkedCount", "uncheckedCount", "indeterminateCount",
                            "scrollOffset", "scrollableHeight", "scrollViewportHeight",
                            "scrollHorizontalOffset", "scrollCount", "scrollAtTop", "scrollAtBottom",
                            "denseMode", "denseSwapped", "legacyScrollCheckpoint", "denseFieldValues",
                            "denseFieldChangeCounts", "denseChecked", "denseCheckedCounts",
                            "denseUncheckedCounts", "denseButtonInvocationCounts",
                            "choicesReady", "choicesExpanded", "choicesSelected", "choicesComboSelected",
                            "choicesComboSelectionCount", "choicesSelectionCount", "choicesExpansionCount", "choicesCollapseCount",
                        )
                        print(
                            "Owned fixture state at failure: "
                            + json.dumps({key: snapshot.get(key) for key in keys}),
                            file=sys.stderr, flush=True,
                        )
                test.check_returncode()
            if args.visual_click:
                report = json.loads((directory / "visual-click-report.json").read_text(encoding="utf-8"))
                if report.get("status") != "passed" or report.get("clicks") != 1 or report.get("centerVerified") is not True:
                    raise AssertionError("Visual click did not independently verify the owned canary")
                print("NATIVE_VISUAL_CLICK_REPORT " + json.dumps(report), flush=True)
                return 0
            if args.capture_only:
                await_snapshot(snapshot_path, fixture, "ordinary-seed", 5)
                print("PASS: owned capture, marker pixels, wrong-PID/minimized refusal, explicit restore and unchanged controls.")
                return 0
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
            for key in ("auxiliaryOpen", "secondWindowOpen"):
                if final.get(key) is not False:
                    raise AssertionError(f"Owned extra window was not closed: {key}")
            if final.get("invocationCount") != 1:
                raise AssertionError("Owned fixture button must be invoked exactly once")
            expected_control_state = {"selected": True, "menuExpanded": False, "selectionCount": 1, "expansionCount": 1, "collapseCount": 1}
            expected_control_state.update(choicesExpanded=False, choicesSelected=True,
                                          choicesComboSelected=True, choicesSelectionCount=1,
                                          choicesComboSelectionCount=1, choicesExpansionCount=3,
                                          choicesCollapseCount=3)
            for key, expected in expected_control_state.items():
                if final.get(key) != expected:
                    raise AssertionError(f"Owned fixture control-state receipt mismatch: {key}")
            expected_checkbox_state = {
                "checkboxChecked": None, "checkboxState": "indeterminate",
                "checkedCount": 1, "uncheckedCount": 1, "indeterminateCount": 1,
            }
            for key, expected in expected_checkbox_state.items():
                if key not in final or final[key] != expected:
                    raise AssertionError(f"Owned checkbox desired-state receipt mismatch: {key}")
            expected_dense_state = {
                "denseMode": True, "denseSwapped": True,
                "denseFieldValues": ["", "region-B-seed", "region-C-seed", "region-D-seed"],
                "denseFieldChangeCounts": [3, 0, 0, 0],
                "denseChecked": [False, False, False, False],
                "denseCheckedCounts": [1, 0, 0, 0],
                "denseUncheckedCounts": [1, 0, 0, 0],
                "denseButtonInvocationCounts": [0, 0, 0, 0],
            }
            for key, expected in expected_dense_state.items():
                if key not in final or final[key] != expected:
                    raise AssertionError(f"Owned dense-region receipt mismatch: {key}")
            # The old panel is removed, so its ScrollViewer may lose layout.
            # This named checkpoint was recorded while it was still visible;
            # every other legacy field/counter above remains a live readback.
            scroll_receipt = final.get("legacyScrollCheckpoint")
            if not isinstance(scroll_receipt, dict):
                raise AssertionError("Owned legacy scroll checkpoint is missing")
            if (
                scroll_receipt.get("scrollCount") != 2
                or scroll_receipt.get("scrollOffset") != 0
                or scroll_receipt.get("scrollHorizontalOffset") != 0
                or scroll_receipt.get("scrollAtTop") is not True
                or not isinstance(scroll_receipt.get("scrollableHeight"), (int, float))
                or scroll_receipt["scrollableHeight"] <= 0
            ):
                raise AssertionError("Owned scroll area must move down/up once each, preserve horizontal position and finish at top")
            if args.controls_only:
                print("PASS: real UIA confirmation/scoped-region actions and readback; no capture checks or normal binary build.")
            else:
                print("PASS: real UIA confirmation/scoped-region actions and readback; owned single-window WGC marker pixels and wrong-PID refusal through the current normal helper binary.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Native UIA smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
