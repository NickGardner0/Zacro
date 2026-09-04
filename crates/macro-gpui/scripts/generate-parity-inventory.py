#!/usr/bin/env python3
"""Generate the pinned Macro source inventory used by parity tests."""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path


SCRIPT = Path(__file__).resolve()
ZED = SCRIPT.parents[3]
MACRO = ZED.parent / "macro"
OUTPUT = SCRIPT.parent.parent / "SOURCE_INVENTORY.json"


def git_revision(repo: Path) -> str:
    return subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=repo, text=True
    ).strip()


def source_routes() -> list[str]:
    source = (MACRO / "apps/web/src/routes/Root.tsx").read_text()
    return sorted(set(re.findall(r"\bpath:\s*['\"]([^'\"]+)['\"]", source)))


def websocket_events() -> list[str]:
    source = (MACRO / "apps/web/src/lib/queries/sync/SyncProvider.tsx").read_text()
    literals = re.findall(r"\.with\(\{\s*type:\s*['\"]([^'\"]+)['\"]", source)
    constants = {
        "AGENT_SESSION_LOG_EVENT": "agent_session_log",
        "AGENT_SESSION_RENAMED_EVENT": "agent_session_renamed",
    }
    for constant, value in constants.items():
        if re.search(rf"\.with\(\{{\s*type:\s*{constant}\s*\}}", source):
            literals.append(value)
    return sorted(set(literals))


def generated_sdk_methods() -> dict[str, list[str]]:
    sdk_root = MACRO / "packages/sdk/generated"
    services: dict[str, list[str]] = {}
    for sdk_file in sorted(sdk_root.glob("*/sdk.gen.ts")):
        source = sdk_file.read_text()
        methods = re.findall(
            r"^\s+public\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*<", source, re.MULTILINE
        )
        services[sdk_file.parent.name] = sorted(set(methods))
    return services


def command_registrations() -> list[dict[str, object]]:
    """Inventory command tokens at their concrete web-client registrations."""
    web_root = MACRO / "apps/web/src"
    registrations: dict[str, set[str]] = {}
    token_pattern = re.compile(
        r"hotkeyToken\s*:\s*(?:blockHotkeyToken\(\s*)?TOKENS\."
        r"([A-Za-z_$][A-Za-z0-9_$.]*)"
    )
    literal_pattern = re.compile(r"hotkeyToken\s*:\s*['\"]([^'\"]+)['\"]")

    for source_file in sorted((*web_root.rglob("*.ts"), *web_root.rglob("*.tsx"))):
        if (
            not source_file.is_file()
            or
            source_file.name.endswith((".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx"))
            or "generated" in source_file.parts
        ):
            continue
        source = source_file.read_text(errors="replace")
        relative = str(source_file.relative_to(MACRO))
        for pattern in (token_pattern, literal_pattern):
            for match in pattern.finditer(source):
                token = match.group(1).rstrip(".)")
                line = source.count("\n", 0, match.start()) + 1
                registrations.setdefault(token, set()).add(f"{relative}:{line}")

    return [
        {"token": token, "locations": sorted(locations)}
        for token, locations in sorted(registrations.items())
    ]


def settings_surfaces() -> list[dict[str, str]]:
    source = (
        MACRO / "apps/web/src/lib/core/constant/settingsTabsConfig.tsx"
    ).read_text()
    match = re.search(
        r"const SETTINGS_TAB_SLUGS:[^=]+\s*=\s*\{(?P<body>.*?)\n\};",
        source,
        re.DOTALL,
    )
    if match is None:
        raise RuntimeError("could not find SETTINGS_TAB_SLUGS")
    entries = re.findall(
        r"^\s*(?:['\"](?P<quoted>[^'\"]+)['\"]|(?P<plain>[A-Za-z][A-Za-z0-9]*))"
        r"\s*:\s*['\"](?P<slug>[^'\"]+)['\"],",
        match.group("body"),
        re.MULTILINE,
    )
    return sorted(
        ({"tab": quoted or plain, "slug": slug} for quoted, plain, slug in entries),
        key=lambda entry: entry["slug"],
    )


def main() -> None:
    feature_root = MACRO / "apps/web/src/features"
    sdk_root = MACRO / "packages/sdk/generated"
    inventory = {
        "schema_version": 2,
        "macro_revision": git_revision(MACRO),
        "zed_revision": git_revision(ZED),
        "sources": {
            "routes": source_routes(),
            "feature_directories": sorted(
                path.name for path in feature_root.iterdir() if path.is_dir()
            ),
            "generated_sdk_files": sorted(
                str(path.relative_to(MACRO)) for path in sdk_root.rglob("*") if path.is_file()
            ),
            "generated_sdk_methods": generated_sdk_methods(),
            "command_registrations": command_registrations(),
            "settings_surfaces": settings_surfaces(),
            "websocket_events": websocket_events(),
        },
    }
    OUTPUT.write_text(json.dumps(inventory, indent=2) + "\n")


if __name__ == "__main__":
    main()
