#!/usr/bin/env python3
"""Build, measure, and attest Macro's real GPUI interaction latency."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import shutil
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
BASELINE_PATH = REPO_ROOT / "crates/macro-gpui/PARITY_BASELINE.json"
SCHEMA = "macro.gpui.interaction-performance-attestation.v1"
RUNNER_SCHEMA = "macro.gpui.interaction-performance.v1"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for block in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def display_path(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def build_and_stage_release_runner() -> Path:
    completed = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "-p",
            "macro_app",
            "--features",
            "visual-tests",
            "--bin",
            "macro_visual_test_runner",
        ],
        cwd=REPO_ROOT,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError("could not build the release GPUI interaction runner")

    suffix = ".exe" if os.name == "nt" else ""
    built = REPO_ROOT / f"target/release/macro_visual_test_runner{suffix}"
    if not built.is_file():
        raise FileNotFoundError(f"Cargo did not produce {built}")

    staged = (
        REPO_ROOT
        / f"target/macro-gpui/performance-runner/release/macro_visual_test_runner{suffix}"
    )
    staged.parent.mkdir(parents=True, exist_ok=True)
    temporary = staged.with_name(f".{staged.name}.{os.getpid()}.tmp")
    try:
        shutil.copy2(built, temporary)
        temporary.chmod(built.stat().st_mode)
        os.replace(temporary, staged)
    finally:
        temporary.unlink(missing_ok=True)
    return staged.resolve()


def validate_case(
    report: dict[str, object],
    *,
    theme: str,
    iterations: int,
    budget_p95_ms: float,
) -> list[str]:
    failures: list[str] = []
    input_to_paint = report.get("input_to_paint") or {}
    network = report.get("network") or {}
    if report.get("schema") != RUNNER_SCHEMA:
        failures.append(f"{theme}: unexpected runner schema {report.get('schema')!r}")
    if report.get("profile") != "release":
        failures.append(f"{theme}: benchmark did not use a release binary")
    if report.get("theme") != theme:
        failures.append(f"{theme}: runner reported theme {report.get('theme')!r}")
    if report.get("measurement_boundary") != "mouse_down_to_scene_submitted":
        failures.append(f"{theme}: interaction did not measure through scene submission")
    if report.get("measured_iterations") != iterations:
        failures.append(f"{theme}: runner used the wrong iteration count")
    if input_to_paint.get("sample_count") != iterations:
        failures.append(
            f"{theme}: expected {iterations} latency samples, got "
            f"{input_to_paint.get('sample_count')}"
        )
    p95_ms = float(input_to_paint.get("p95_ms", float("inf")))
    if p95_ms > budget_p95_ms:
        failures.append(
            f"{theme}: input-to-paint p95 {p95_ms:.3f} ms exceeds "
            f"{budget_p95_ms:.3f} ms"
        )
    if network.get("top_level_request_delta") != 0:
        failures.append(
            f"{theme}: local selection caused "
            f"{network.get('top_level_request_delta')} top-level requests"
        )
    if report.get("passed") is not True:
        failures.append(f"{theme}: runner marked the interaction case failed")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runner", type=Path, help="prebuilt release runner")
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("target/macro-gpui/performance-current"),
    )
    parser.add_argument("--iterations", type=int, default=120)
    parser.add_argument("--warmup", type=int, default=10)
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("--iterations must be positive")
    if args.warmup < 0:
        parser.error("--warmup must be non-negative")

    baseline = json.loads(BASELINE_PATH.read_text())
    budget_p95_ms = float(
        baseline["performance_budgets"]["cached_input_to_paint_p95_ms"]
    )
    runner = (
        args.runner.resolve()
        if args.runner is not None
        else build_and_stage_release_runner()
    )
    if not runner.is_file():
        parser.error(f"runner does not exist: {runner}")

    output_dir = (REPO_ROOT / args.output_dir).resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    cases: list[dict[str, object]] = []
    failures: list[str] = []
    for theme_arg, theme_name in (("light", "One Light"), ("dark", "One Dark")):
        stem = f"inbox-populated-one-{theme_arg}"
        report_path = output_dir / f"{stem}.performance.json"
        image_path = output_dir / f"{stem}.png"
        accessibility_path = output_dir / f"{stem}.accessibility.json"
        completed = subprocess.run(
            [
                str(runner),
                "--theme",
                theme_arg,
                "--surface",
                "inbox",
                "--state",
                "populated",
                "--output",
                str(image_path),
                "--accessibility-output",
                str(accessibility_path),
                "--performance-output",
                str(report_path),
                "--performance-iterations",
                str(args.iterations),
                "--performance-warmup",
                str(args.warmup),
                "--performance-budget-p95-ms",
                str(budget_p95_ms),
            ],
            cwd=REPO_ROOT,
            check=False,
        )
        if completed.returncode != 0:
            failures.append(f"{theme_name}: runner exited {completed.returncode}")
            continue
        if not report_path.is_file():
            failures.append(f"{theme_name}: runner did not write {report_path}")
            continue
        report = json.loads(report_path.read_text())
        failures.extend(
            validate_case(
                report,
                theme=theme_name,
                iterations=args.iterations,
                budget_p95_ms=budget_p95_ms,
            )
        )
        cases.append(
            {
                "theme": theme_name,
                "report": report,
                "artifacts": {
                    "report": display_path(report_path),
                    "report_sha256": sha256(report_path),
                    "image": display_path(image_path),
                    "image_sha256": sha256(image_path),
                    "accessibility": display_path(accessibility_path),
                    "accessibility_sha256": sha256(accessibility_path),
                },
            }
        )

    attestation = {
        "schema": SCHEMA,
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "source_revisions": baseline["source_revisions"],
        "host": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
        },
        "runner": {
            "path": display_path(runner),
            "sha256": sha256(runner),
            "profile": "release",
        },
        "budget": {"cached_input_to_paint_p95_ms": budget_p95_ms},
        "iterations": args.iterations,
        "warmup_iterations": args.warmup,
        "cases": cases,
        "failures": failures,
        "passed": not failures and len(cases) == 2,
    }
    attestation_path = output_dir / "interaction-performance-attestation.json"
    attestation_path.write_text(json.dumps(attestation, indent=2) + "\n")
    print(f"GPUI interaction attestation saved to {attestation_path}")
    if failures:
        for failure in failures:
            print(f"FAIL: {failure}")
        return 1
    for case in cases:
        report = case["report"]
        latency = report["input_to_paint"]
        print(
            f"PASS: {case['theme']} p50 {latency['p50_ms']:.3f} ms, "
            f"p95 {latency['p95_ms']:.3f} ms"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
