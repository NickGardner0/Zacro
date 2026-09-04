#!/usr/bin/env python3
"""Capture sanitized SolidJS references through Macro's seeded local E2E stack."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
MACRO_GPUI_ROOT = REPO_ROOT / "crates/macro-gpui"
BASELINE_PATH = MACRO_GPUI_ROOT / "PARITY_BASELINE.json"
FIXTURE_MANIFEST_PATH = MACRO_GPUI_ROOT / "PARITY_FIXTURE_MANIFEST.json"
PLAN_PATH = MACRO_GPUI_ROOT / "SOLID_REFERENCE_CAPTURE_PLAN.json"
HARNESS_ROOT = MACRO_GPUI_ROOT / "solid-reference"
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for block in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def png_dimensions(path: Path) -> tuple[int, int]:
    with path.open("rb") as image:
        signature = image.read(8)
        length = struct.unpack(">I", image.read(4))[0]
        chunk = image.read(4)
        if signature != PNG_SIGNATURE or chunk != b"IHDR" or length < 8:
            raise ValueError(f"{path} is not a valid PNG")
        return struct.unpack(">II", image.read(8))


def default_macro_root() -> Path:
    configured = os.environ.get("MACRO_SOURCE_ROOT")
    if configured:
        return Path(configured).expanduser().resolve()
    return (REPO_ROOT.parent / "macro").resolve()


def validate_plan(macro_root: Path) -> tuple[dict[str, object], dict[str, object]]:
    baseline = json.loads(BASELINE_PATH.read_text())
    manifest = json.loads(FIXTURE_MANIFEST_PATH.read_text())
    plan = json.loads(PLAN_PATH.read_text())
    if plan.get("schema") != "macro.gpui.solid-reference-plan.v1":
        raise ValueError("SolidJS reference plan has an unexpected schema")
    expected_revision = baseline["source_revisions"]["macro"]
    if plan.get("macro_revision") != expected_revision:
        raise ValueError("SolidJS reference plan does not use the pinned Macro revision")

    if not macro_root.joinpath(".git").exists():
        raise FileNotFoundError(f"Macro checkout is missing at {macro_root}")
    actual_revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=macro_root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if actual_revision != expected_revision:
        raise ValueError(
            f"Macro checkout is {actual_revision}; expected pinned {expected_revision}"
        )

    candidate_states = manifest["gpui_capture_runner"]["states"]
    seen: set[tuple[str, str]] = set()
    captures = plan.get("captures") or []
    if not captures:
        raise ValueError("SolidJS reference plan has no captures")
    for capture in captures:
        key = (capture.get("surface"), capture.get("state"))
        if key in seen:
            raise ValueError(f"duplicate SolidJS reference capture {key}")
        seen.add(key)
        surface, state = key
        if state not in candidate_states.get(surface, []):
            raise ValueError(f"SolidJS reference {surface}/{state} has no GPUI pair")
        if not str(capture.get("route") or "").startswith("/"):
            raise ValueError(f"SolidJS reference {surface}/{state} has no app route")
        if not capture.get("wait_selector"):
            raise ValueError(f"SolidJS reference {surface}/{state} has no wait selector")
    if plan.get("themes") != ["light", "dark"]:
        raise ValueError("SolidJS reference plan must capture light and dark")
    return baseline, plan


def validate_capture_directory(
    output_dir: Path, plan: dict[str, object]
) -> list[dict[str, object]]:
    viewport = plan["viewport"]
    screenshot_scale = (
        int(viewport["device_scale_factor"])
        if viewport["screenshot_scale"] == "device"
        else 1
    )
    expected_size = (
        int(viewport["width"]) * screenshot_scale,
        int(viewport["height"]) * screenshot_scale,
    )
    expected_names = {"solid-reference-attestation.json"}
    cases: list[dict[str, object]] = []
    for capture in plan["captures"]:
        for theme in plan["themes"]:
            stem = f"{capture['surface']}-{capture['state']}-one-{theme}"
            image_path = output_dir / f"{stem}.png"
            metadata_path = output_dir / f"{stem}.json"
            expected_names.update({image_path.name, metadata_path.name})
            if not image_path.is_file() or not metadata_path.is_file():
                raise FileNotFoundError(f"missing SolidJS reference artifacts for {stem}")
            dimensions = png_dimensions(image_path)
            if dimensions != expected_size:
                raise ValueError(
                    f"{image_path} is {dimensions}, expected {expected_size}"
                )
            metadata = json.loads(metadata_path.read_text())
            if metadata.get("schema") != "macro.gpui.solid-reference-capture.v1":
                raise ValueError(f"{metadata_path} has an unexpected schema")
            if metadata.get("surface") != capture["surface"]:
                raise ValueError(f"{metadata_path} reports the wrong surface")
            if metadata.get("state") != capture["state"]:
                raise ValueError(f"{metadata_path} reports the wrong state")
            if metadata.get("macro_revision") != plan["macro_revision"]:
                raise ValueError(f"{metadata_path} reports the wrong Macro revision")
            if metadata.get("fixture_profile") != plan["fixture_profile"]:
                raise ValueError(f"{metadata_path} reports the wrong fixture profile")
            if metadata.get("route") != capture["route"]:
                raise ValueError(f"{metadata_path} reports the wrong production route")
            if metadata.get("viewport") != viewport:
                raise ValueError(f"{metadata_path} reports the wrong viewport")
            expected_theme = "Macro Light" if theme == "light" else "Macro Dark"
            if metadata.get("theme") != expected_theme:
                raise ValueError(f"{metadata_path} reports the wrong theme")
            expected_light = "true" if theme == "light" else "false"
            if metadata.get("dom", {}).get("theme_light") != expected_light:
                raise ValueError(f"{metadata_path} did not apply {expected_theme}")
            if not metadata.get("text_sha256") or metadata.get(
                "visible_text_length", 0
            ) < 1:
                raise ValueError(f"{metadata_path} has no visible-content attestation")
            cases.append(
                {
                    "surface": capture["surface"],
                    "state": capture["state"],
                    "theme": expected_theme,
                    "image": image_path.name,
                    "image_sha256": sha256(image_path),
                    "metadata": metadata_path.name,
                    "metadata_sha256": sha256(metadata_path),
                    "dimensions": {"width": dimensions[0], "height": dimensions[1]},
                    "dom": metadata["dom"],
                    "text_sha256": metadata["text_sha256"],
                }
            )
    unexpected = sorted(
        path.name
        for path in output_dir.iterdir()
        if path.is_file()
        and path.suffix in {".png", ".json"}
        and path.name not in expected_names
    )
    if unexpected:
        raise ValueError(f"unexpected SolidJS reference artifacts: {unexpected}")
    return cases


def prepare_harness(macro_root: Path, plan_path: Path) -> Path:
    web_root = macro_root / "apps/web"
    harness = Path(tempfile.mkdtemp(prefix=".macro-gpui-reference-", dir=web_root))
    shutil.copy2(HARNESS_ROOT / "playwright.config.ts", harness / "playwright.config.ts")
    shutil.copy2(
        HARNESS_ROOT / "solid-reference.spec.ts", harness / "solid-reference.spec.ts"
    )
    shutil.copy2(plan_path, harness / "capture-plan.json")
    return harness


def prerequisites(macro_root: Path) -> list[str]:
    missing: list[str] = []
    if not os.environ.get("IN_NIX_SHELL") and shutil.which("nix") is None:
        missing.append("nix (required to enter Macro's repository development shell)")
    if shutil.which("docker") is None:
        missing.append("docker (required by Macro's isolated local service stack)")
    if shutil.which("bun") is None:
        missing.append("bun (required by Macro's Playwright harness)")
    runner = macro_root / "tooling/scripts/run-local-e2e.sh"
    if not runner.is_file():
        missing.append(str(runner))
    return missing


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--macro-root", type=Path, default=default_macro_root())
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("target/macro-gpui/solid-reference-current"),
    )
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--keep-harness", action="store_true")
    args = parser.parse_args()
    if args.prepare_only and args.validate_only:
        parser.error("--prepare-only and --validate-only are mutually exclusive")

    macro_root = args.macro_root.expanduser().resolve()
    baseline, plan = validate_plan(macro_root)
    output_dir = (REPO_ROOT / args.output_dir).resolve()
    missing = prerequisites(macro_root)
    if args.prepare_only:
        print(
            f"SolidJS capture plan validated: {len(plan['captures']) * len(plan['themes'])} artifacts"
        )
        if missing:
            print("Runtime prerequisites unavailable:")
            for item in missing:
                print(f"- {item}")
        else:
            print("All runtime prerequisites are available")
        return 0
    if args.validate_only:
        cases = validate_capture_directory(output_dir, plan)
        print(f"Validated {len(cases)} SolidJS reference captures in {output_dir}")
        return 0
    if missing:
        raise RuntimeError("SolidJS reference capture cannot start: " + "; ".join(missing))

    output_dir.parent.mkdir(parents=True, exist_ok=True)
    staging_dir = Path(
        tempfile.mkdtemp(prefix=f".{output_dir.name}.", dir=output_dir.parent)
    )
    harness = prepare_harness(macro_root, PLAN_PATH)
    try:
        env = os.environ.copy()
        env.update(
            {
                "SOLID_REFERENCE_CAPTURE_PLAN": str(harness / "capture-plan.json"),
                "SOLID_REFERENCE_OUTPUT_DIR": str(staging_dir),
                "SOLID_REFERENCE_TEST_OUTPUT": str(staging_dir / "test-results"),
            }
        )
        completed = subprocess.run(
            [
                str(macro_root / "tooling/scripts/run-local-e2e.sh"),
                "--suite",
                "web",
                "--",
                "--config",
                str(harness / "playwright.config.ts"),
            ],
            cwd=macro_root,
            env=env,
            check=False,
        )
        if completed.returncode != 0:
            raise RuntimeError(
                f"Macro seeded SolidJS capture exited {completed.returncode}"
            )
        cases = validate_capture_directory(staging_dir, plan)
        attestation = {
            "schema": "macro.gpui.solid-reference-attestation.v1",
            "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "fixture_profile": plan["fixture_profile"],
            "source_revisions": baseline["source_revisions"],
            "plan_sha256": sha256(PLAN_PATH),
            "harness": {
                "config_sha256": sha256(HARNESS_ROOT / "playwright.config.ts"),
                "spec_sha256": sha256(HARNESS_ROOT / "solid-reference.spec.ts"),
            },
            "expected_capture_count": len(plan["captures"]) * len(plan["themes"]),
            "capture_count": len(cases),
            "cases": cases,
            "passed": True,
        }
        (staging_dir / "solid-reference-attestation.json").write_text(
            json.dumps(attestation, indent=2) + "\n"
        )
        if output_dir.exists():
            backup = output_dir.with_name(
                f"{output_dir.name}.previous-{dt.datetime.now().strftime('%Y%m%d-%H%M%S')}"
            )
            output_dir.rename(backup)
            print(f"Previous reference corpus retained at {backup}")
        staging_dir.rename(output_dir)
        print(f"Captured and attested {len(cases)} SolidJS references in {output_dir}")
        return 0
    finally:
        if not args.keep_harness:
            shutil.rmtree(harness, ignore_errors=True)
        elif harness.exists():
            print(f"Temporary Macro Playwright harness retained at {harness}")
        if staging_dir.exists() and staging_dir != output_dir:
            shutil.rmtree(staging_dir, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
