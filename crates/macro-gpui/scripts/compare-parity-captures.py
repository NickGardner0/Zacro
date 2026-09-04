#!/usr/bin/env python3
"""Build attestable side-by-side SolidJS and Zed/GPUI review sheets."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


REPO_ROOT = Path(__file__).resolve().parents[3]
PLAN_PATH = REPO_ROOT / "crates/macro-gpui/SOLID_REFERENCE_CAPTURE_PLAN.json"
BASELINE_PATH = REPO_ROOT / "crates/macro-gpui/PARITY_BASELINE.json"
FIXTURE_MANIFEST_PATH = REPO_ROOT / "crates/macro-gpui/PARITY_FIXTURE_MANIFEST.json"
HEADER_HEIGHT = 52
GUTTER = 12


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for block in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def load_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text())


def image_size(path: Path) -> tuple[int, int]:
    with Image.open(path) as image:
        return image.size


def review_sheet(reference: Path, candidate: Path, output: Path, title: str) -> None:
    with Image.open(reference).convert("RGB") as left, Image.open(candidate).convert(
        "RGB"
    ) as right:
        if left.size != right.size:
            raise ValueError(
                f"capture dimensions differ for {title}: {left.size} vs {right.size}"
            )
        width, height = left.size
        canvas = Image.new(
            "RGB",
            (width * 2 + GUTTER, height + HEADER_HEIGHT),
            color=(30, 30, 32),
        )
        canvas.paste(left, (0, HEADER_HEIGHT))
        canvas.paste(right, (width + GUTTER, HEADER_HEIGHT))
        draw = ImageDraw.Draw(canvas)
        try:
            font = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 18)
        except OSError:
            font = ImageFont.load_default()
        draw.text((16, 8), title, fill=(235, 235, 238), font=font)
        draw.text(
            (16, 29),
            "Macro SolidJS reference",
            fill=(178, 178, 184),
            font=font,
        )
        draw.text(
            (width + GUTTER + 16, 29),
            "Zed-styled Macro GPUI candidate",
            fill=(178, 178, 184),
            font=font,
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        canvas.save(output)


def validate_candidate_manifest(
    candidate_dir: Path, expected_count: int
) -> dict[str, object]:
    manifest_path = candidate_dir / "candidate-manifest.json"
    manifest = load_json(manifest_path)
    if manifest.get("schema") != "macro.gpui.visual-candidates.v2":
        raise ValueError(f"{manifest_path} has an unexpected schema")
    if manifest.get("capture_count", 0) < expected_count:
        raise ValueError(f"{manifest_path} does not contain the required candidates")
    baseline = load_json(BASELINE_PATH)
    if manifest.get("source_revisions") != baseline.get("source_revisions"):
        raise ValueError(f"{manifest_path} does not attest the pinned source revisions")
    if manifest.get("source_manifest_sha256") != sha256(FIXTURE_MANIFEST_PATH):
        raise ValueError(f"{manifest_path} does not attest the current fixture manifest")
    runner = manifest.get("runner") or {}
    if not runner.get("built_by_capture_script") or not runner.get("sha256"):
        raise ValueError(f"{manifest_path} has no reproducible runner provenance")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--reference-dir",
        type=Path,
        default=Path("target/macro-gpui/solid-reference-current"),
    )
    parser.add_argument(
        "--candidate-dir",
        type=Path,
        default=Path("target/macro-gpui/visual-current"),
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("target/macro-gpui/parity-comparison-current"),
    )
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()

    plan = load_json(PLAN_PATH)
    baseline = load_json(BASELINE_PATH)
    pair_count = len(plan["captures"]) * len(plan["themes"])
    reference_dir = (REPO_ROOT / args.reference_dir).resolve()
    candidate_dir = (REPO_ROOT / args.candidate_dir).resolve()
    output_dir = (REPO_ROOT / args.output_dir).resolve()
    candidate_manifest = validate_candidate_manifest(candidate_dir, pair_count)
    missing_reference = not reference_dir.joinpath(
        "solid-reference-attestation.json"
    ).is_file()
    if args.prepare_only:
        print(f"GPUI candidates available for all {pair_count} planned comparison pairs")
        if missing_reference:
            print(
                "SolidJS reference attestation is not present; run capture-solid-reference.py on a Macro local-stack host"
            )
        else:
            print("SolidJS reference attestation is available")
        return 0
    if missing_reference:
        raise FileNotFoundError(
            f"missing {reference_dir / 'solid-reference-attestation.json'}"
        )

    reference_attestation = load_json(
        reference_dir / "solid-reference-attestation.json"
    )
    if reference_attestation.get("schema") != "macro.gpui.solid-reference-attestation.v1":
        raise ValueError("SolidJS reference attestation has an unexpected schema")
    if reference_attestation.get("passed") is not True:
        raise ValueError("SolidJS reference attestation is not passing")
    if reference_attestation.get("source_revisions") != baseline.get("source_revisions"):
        raise ValueError("reference and candidate source revision contract has drifted")
    if reference_attestation.get("plan_sha256") != sha256(PLAN_PATH):
        raise ValueError("SolidJS reference attestation does not use the current plan")
    if reference_attestation.get("capture_count") != pair_count:
        raise ValueError("SolidJS reference attestation has incomplete capture coverage")

    candidate_cases = {
        (case["surface"], case["state"], case["theme"]): case
        for case in candidate_manifest.get("captures", [])
    }

    output_dir.parent.mkdir(parents=True, exist_ok=True)
    staging_dir = Path(
        tempfile.mkdtemp(prefix=f".{output_dir.name}.", dir=output_dir.parent)
    )
    pairs: list[dict[str, object]] = []
    try:
        for capture in plan["captures"]:
            for theme in plan["themes"]:
                stem = f"{capture['surface']}-{capture['state']}-one-{theme}"
                reference_image = reference_dir / f"{stem}.png"
                reference_metadata = reference_dir / f"{stem}.json"
                candidate_image = candidate_dir / f"{stem}.png"
                candidate_accessibility = candidate_dir / f"{stem}.a11y.json"
                for artifact in [
                    reference_image,
                    reference_metadata,
                    candidate_image,
                    candidate_accessibility,
                ]:
                    if not artifact.is_file():
                        raise FileNotFoundError(f"missing comparison artifact {artifact}")
                if image_size(reference_image) != image_size(candidate_image):
                    raise ValueError(f"capture size mismatch for {stem}")
                candidate_case = candidate_cases.get(
                    (capture["surface"], capture["state"], theme)
                )
                if candidate_case is None:
                    raise ValueError(f"candidate manifest has no case for {stem}")
                if candidate_case.get("path") != candidate_image.name:
                    raise ValueError(f"candidate manifest reports the wrong image for {stem}")
                if candidate_case.get("sha256") != sha256(candidate_image):
                    raise ValueError(f"candidate image hash mismatch for {stem}")
                candidate_accessibility_case = candidate_case.get("accessibility") or {}
                if candidate_accessibility_case.get("path") != candidate_accessibility.name:
                    raise ValueError(
                        f"candidate manifest reports the wrong accessibility tree for {stem}"
                    )
                if candidate_accessibility_case.get("sha256") != sha256(
                    candidate_accessibility
                ):
                    raise ValueError(f"candidate accessibility hash mismatch for {stem}")

                sheet = staging_dir / f"{stem}.side-by-side.png"
                review_sheet(
                    reference_image,
                    candidate_image,
                    sheet,
                    f"{capture['surface']} / {capture['state']} / {theme}",
                )
                reference = load_json(reference_metadata)
                candidate_a11y = load_json(candidate_accessibility)
                candidate_frame = candidate_a11y.get("frame") or {}
                pairs.append(
                    {
                        "surface": capture["surface"],
                        "state": capture["state"],
                        "theme": theme,
                        "reference": {
                            "image_sha256": sha256(reference_image),
                            "metadata_sha256": sha256(reference_metadata),
                            "text_sha256": reference["text_sha256"],
                            "dom_counts": reference["dom"]["counts"],
                            "layout": reference["dom"]["layout"],
                        },
                        "candidate": {
                            "image_sha256": sha256(candidate_image),
                            "accessibility_sha256": sha256(candidate_accessibility),
                            "node_count": candidate_frame.get("node_count"),
                            "tab_stop_count": candidate_frame.get("tab_stop_count"),
                        },
                        "review_sheet": sheet.name,
                        "review_sheet_sha256": sha256(sheet),
                        "review_policy": "manual behavioral and information-hierarchy review; pixel equality is not expected across design systems",
                    }
                )

        attestation = {
            "schema": "macro.gpui.visual-parity-comparison.v1",
            "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "source_revisions": baseline["source_revisions"],
            "plan_sha256": sha256(PLAN_PATH),
            "reference_attestation_sha256": sha256(
                reference_dir / "solid-reference-attestation.json"
            ),
            "candidate_attestation_sha256": sha256(
                candidate_dir / "candidate-manifest.json"
            ),
            "candidate_runner": candidate_manifest.get("runner"),
            "pair_count": len(pairs),
            "pairs": pairs,
            "automated_gate": "artifact completeness, source pins, dimensions, redaction attestation, and accessibility availability",
            "manual_gate": "feature presence, hierarchy, density, interaction affordance, and Zed design quality",
            "passed": len(pairs) == pair_count,
        }
        (staging_dir / "visual-parity-comparison.json").write_text(
            json.dumps(attestation, indent=2) + "\n"
        )
        if output_dir.exists():
            backup = output_dir.with_name(
                f"{output_dir.name}.previous-{dt.datetime.now().strftime('%Y%m%d-%H%M%S')}"
            )
            output_dir.rename(backup)
            print(f"Previous comparison corpus retained at {backup}")
        staging_dir.rename(output_dir)
        print(f"Generated {len(pairs)} side-by-side parity review sheets in {output_dir}")
        return 0
    finally:
        if staging_dir.exists():
            for root, directories, files in os.walk(staging_dir, topdown=False):
                for file in files:
                    Path(root, file).unlink()
                for directory in directories:
                    Path(root, directory).rmdir()
            staging_dir.rmdir()


if __name__ == "__main__":
    raise SystemExit(main())
