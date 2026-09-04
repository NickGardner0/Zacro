#!/usr/bin/env python3
"""Capture and attest the deterministic GPUI parity-candidate matrix."""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
from pathlib import Path


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
BASELINE_PATH = Path(__file__).resolve().parents[1] / "PARITY_BASELINE.json"


def png_dimensions(path: Path) -> tuple[int, int]:
    with path.open("rb") as image:
        signature = image.read(8)
        length = struct.unpack(">I", image.read(4))[0]
        chunk = image.read(4)
        if signature != PNG_SIGNATURE or chunk != b"IHDR" or length < 8:
            raise ValueError(f"{path} is not a valid PNG with an IHDR header")
        return struct.unpack(">II", image.read(8))


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for block in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def build_and_stage_runner() -> Path:
    """Build the current visual runner and copy it to an immutable run path.

    `cargo check` does not refresh executable artifacts, and another Cargo
    invocation may replace the top-level target symlink while captures are
    launching in parallel. Building here prevents stale-source attestations;
    staging a private executable prevents target-directory churn from
    invalidating an in-progress matrix.
    """

    completed = subprocess.run(
        [
            "cargo",
            "build",
            "-p",
            "macro_app",
            "--features",
            "visual-tests",
            "--bin",
            "macro_visual_test_runner",
        ],
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError("could not build the current GPUI visual-test runner")

    suffix = ".exe" if os.name == "nt" else ""
    built = Path(f"target/debug/macro_visual_test_runner{suffix}")
    if not built.is_file():
        raise FileNotFoundError(f"Cargo did not produce {built}")

    staged = Path(f"target/macro-gpui/visual-runner/macro_visual_test_runner{suffix}")
    staged.parent.mkdir(parents=True, exist_ok=True)
    temporary = staged.with_name(f".{staged.name}.{os.getpid()}.tmp")
    try:
        shutil.copy2(built, temporary)
        temporary.chmod(built.stat().st_mode)
        os.replace(temporary, staged)
    finally:
        temporary.unlink(missing_ok=True)
    return staged.resolve()


def validate_accessibility_tree(
    path: Path, *, surface: str, state: str
) -> dict[str, int]:
    tree = json.loads(path.read_text())
    frame = tree.get("frame") or {}
    nodes = tree.get("nodes") or {}
    if not tree.get("root") or not nodes:
        raise ValueError(f"{path} has no accessibility root or nodes")
    if frame.get("node_count") != len(nodes):
        raise ValueError(
            f"{path} reports {frame.get('node_count')} nodes but contains {len(nodes)}"
        )
    if int(frame.get("tab_stop_count", 0)) < 1:
        raise ValueError(f"{path} has no keyboard tab stops")

    actionable = []
    unlabeled = []
    click_without_focus = []
    technical_labels = []
    for key, node in nodes.items():
        aria = node.get("aria") or {}
        actions = aria.get("on_action") or []
        if "Click" not in actions:
            continue
        actionable.append(key)
        if not any(aria.get(field) for field in ("label", "description", "value")):
            unlabeled.append(key)
        if "Focus" not in actions:
            click_without_focus.append(key)
        label = str(aria.get("label") or "")
        if re.search(
            r"\b[0-9a-f]{8}(?: [0-9a-f]{4}){3} [0-9a-f]{12}\b", label, re.I
        ) or re.search(
            r"\b(?:inbox|tasks|documents|files|agents|calls|crm) item [\w ]* \d+\b",
            label,
            re.I,
        ):
            technical_labels.append(key)
    if not actionable:
        raise ValueError(f"{path} exposes no actionable accessibility nodes")
    if unlabeled:
        raise ValueError(f"{path} has unlabeled actionable nodes: {unlabeled}")
    if click_without_focus:
        raise ValueError(
            f"{path} has pointer actions that cannot receive focus: {click_without_focus}"
        )
    if technical_labels:
        raise ValueError(
            f"{path} exposes implementation ids instead of action names: {technical_labels}"
        )

    if surface == "documents" and state in {
        "inline_editing",
        "accessibility_edit",
        "ime_composition",
        "pointer_editing",
    }:
        inline_editors = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
            and str((node.get("aria") or {}).get("label") or "").startswith("Edit ")
        ]
        if len(inline_editors) != 1:
            raise ValueError(
                f"{path} must expose exactly one named inline document text input"
            )
        inline_editor = inline_editors[0].get("aria") or {}
        inline_actions = inline_editor.get("on_action") or []
        if "Focus" not in inline_actions or "SetValue" not in inline_actions:
            raise ValueError(
                f"{path} inline document text input must support Focus and SetValue"
            )
        if not inline_editor.get("value"):
            raise ValueError(f"{path} inline document text input has no readable value")
        if (
            state == "accessibility_edit"
            and inline_editor.get("value") != "Accessible launch copy · 日本語"
        ):
            raise ValueError(
                f"{path} did not retain the value dispatched through AccessKit SetValue"
            )
        if state == "ime_composition" and inline_editor.get("value") != "日本語":
            raise ValueError(
                f"{path} did not retain the value committed through GPUI's IME input handler"
            )
        if (
            state == "pointer_editing"
            and inline_editor.get("value") != "Pointer edited launch copy"
        ):
            raise ValueError(
                f"{path} did not retain the value entered after the rich-block pointer click"
            )
    if surface == "documents" and state in {"mention_picker", "mention_picker_filtered"}:
        mention_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Mention results"
        ]
        if len(mention_lists) != 1:
            raise ValueError(f"{path} must expose one named document mention result list")
        mention_choices = [
            node
            for node in nodes.values()
            if (
                (node.get("aria") or {}).get("role") == "Button"
                and str((node.get("aria") or {}).get("label") or "").startswith(
                    ("Insert @", "Insert #")
                )
            )
        ]
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in mention_choices
        }
        if state == "mention_picker":
            if len(mention_choices) != 5:
                raise ValueError(f"{path} must expose five typed document mention choices")
            for required in [
                "Insert @Maya Chen",
                "Insert @Rollout checklist",
                "Insert @Ship the native release",
                "Insert @Northwind Labs",
                "Insert #Launch",
            ]:
                if required not in labels:
                    raise ValueError(f"{path} is missing mention choice {required}")
        elif labels != {"Insert @Northwind Labs"}:
            raise ValueError(f"{path} did not filter to the matching typed entity")
        selected_choices = [
            node
            for node in mention_choices
            if (node.get("aria") or {}).get("selected") is True
        ]
        if len(selected_choices) != 1:
            raise ValueError(f"{path} must expose exactly one selected mention result")
        mention_inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
        ]
        if len(mention_inputs) != 1:
            raise ValueError(
                f"{path} exposes raw backend-ID fields before Custom mention; only entity search is allowed"
            )
        search_input = mention_inputs[0].get("aria") or {}
        if "Search people, documents, customers, and tags" not in str(
            search_input.get("label") or search_input.get("placeholder") or ""
        ):
            raise ValueError(f"{path} is missing the named entity mention search input")
        actions = search_input.get("on_action") or []
        if "Focus" not in actions or "SetValue" not in actions:
            raise ValueError(f"{path} entity mention search must support Focus and SetValue")
        expected_value = "northwind" if state == "mention_picker_filtered" else ""
        if search_input.get("value") != expected_value:
            raise ValueError(f"{path} did not expose the filtered mention search value")
    if surface == "documents" and state == "bidirectional_text":
        accessible_text = "\n".join(
            str(value)
            for node in nodes.values()
            for value in (node.get("aria") or {}).values()
            if isinstance(value, str)
        )
        for required in [
            "خطة إطلاق المنتج",
            "סקירת השקה",
            "Product launch plan",
            "status 42%",
        ]:
            if required not in accessible_text:
                raise ValueError(
                    f"{path} is missing bidirectional document content {required!r}"
                )
    if (surface == "documents" and state == "sharing") or (
        surface == "mail" and state == "share"
    ):
        sharing_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Available sharing channels"
        ]
        if len(sharing_lists) != 1:
            raise ValueError(f"{path} must expose one named sharing channel list")
        sharing_choices = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and str((node.get("aria") or {}).get("label") or "").startswith(
                "Share with #"
            )
        ]
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in sharing_choices
        }
        if labels != {"Share with #product", "Share with #leadership"}:
            raise ValueError(f"{path} does not expose the named workspace channels")
        selected_labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in sharing_choices
            if (node.get("aria") or {}).get("selected") is True
        }
        expected_selected = (
            {"Share with #leadership"}
            if surface == "documents"
            else {"Share with #product"}
        )
        if selected_labels != expected_selected:
            raise ValueError(f"{path} does not expose the selected sharing channel")
        if any(
            (node.get("aria") or {}).get("role") == "TextInput"
            for node in nodes.values()
        ):
            raise ValueError(f"{path} exposes raw sharing transport fields")
        if surface == "documents":
            visibility = {
                str((node.get("aria") or {}).get("label") or ""): (
                    node.get("aria") or {}
                ).get("toggled")
                for node in nodes.values()
                if str((node.get("aria") or {}).get("label") or "")
                in {"Private", "Workspace", "Public"}
            }
            if visibility != {
                "Private": "False",
                "Workspace": "True",
                "Public": "False",
            }:
                raise ValueError(f"{path} does not expose the current link visibility")
            if not any(
                (node.get("aria") or {}).get("role") == "Button"
                and (node.get("aria") or {}).get("label") == "Remove"
                for node in nodes.values()
            ):
                raise ValueError(f"{path} is missing the existing channel remove control")
    if surface == "documents" and state == "property_select":
        option_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Property options"
        ]
        if len(option_lists) != 1:
            raise ValueError(f"{path} must expose one named property option list")
        options = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and str((node.get("aria") or {}).get("label") or "").startswith(
                "Set Launch stage to "
            )
        ]
        labels = {
            str((node.get("aria") or {}).get("label") or "") for node in options
        }
        if labels != {
            "Set Launch stage to Planned",
            "Set Launch stage to In review",
            "Set Launch stage to Shipped",
        }:
            raise ValueError(f"{path} does not expose named property options")
        selected = {
            str((node.get("aria") or {}).get("label") or "")
            for node in options
            if (node.get("aria") or {}).get("selected") is True
        }
        if selected != {"Set Launch stage to In review"}:
            raise ValueError(f"{path} does not expose the selected property option")
        if any(
            (node.get("aria") or {}).get("role") == "TextInput"
            for node in nodes.values()
        ):
            raise ValueError(f"{path} exposes a raw property option ID input")
    if surface == "documents" and state == "property_entity":
        entity_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Property entity results"
        ]
        if len(entity_lists) != 1:
            raise ValueError(f"{path} must expose one named property entity list")
        entity_options = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and str((node.get("aria") or {}).get("label") or "").startswith(
                "Set Owner to "
            )
        ]
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in entity_options
        }
        for required in {
            "Set Owner to Maya Chen",
            "Set Owner to Rollout checklist",
            "Set Owner to Ship the native release",
            "Set Owner to Northwind Labs",
        }:
            if required not in labels:
                raise ValueError(f"{path} is missing typed entity property choice {required}")
        selected = {
            str((node.get("aria") or {}).get("label") or "")
            for node in entity_options
            if (node.get("aria") or {}).get("selected") is True
        }
        if selected != {"Set Owner to Maya Chen"}:
            raise ValueError(f"{path} does not expose the selected entity property")
        inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
        ]
        if len(inputs) != 1:
            raise ValueError(f"{path} exposes raw entity reference fields")
        search = inputs[0].get("aria") or {}
        if search.get("label") != "Search people, documents, tasks, and companies":
            raise ValueError(f"{path} is missing the named property entity search")
        if not {"Focus", "SetValue"}.issubset(set(search.get("on_action") or [])):
            raise ValueError(f"{path} property entity search is not editable")
    if surface == "documents" and state == "entity_link":
        link_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Linkable entities"
        ]
        if len(link_lists) != 1:
            raise ValueError(f"{path} must expose one named linkable entity list")
        link_options = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and str((node.get("aria") or {}).get("label") or "").startswith("Link ")
        ]
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in link_options
        }
        for required in {
            "Link Maya Chen",
            "Link Rollout checklist",
            "Link Ship the native release",
            "Link Northwind Labs",
            "Link #product",
            "Link Product launch",
        }:
            if required not in labels:
                raise ValueError(f"{path} is missing typed entity-link choice {required}")
        if "Link Product launch brief" in labels:
            raise ValueError(f"{path} entity-link picker includes source entity")
        selected = [
            node
            for node in link_options
            if (node.get("aria") or {}).get("selected") is True
        ]
        if len(selected) != 1:
            raise ValueError(f"{path} must expose exactly one selected entity-link result")
        inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
        ]
        if len(inputs) != 1:
            raise ValueError(f"{path} exposes raw entity-link transport fields")
        search = inputs[0].get("aria") or {}
        if search.get("label") != "Search workspace entities":
            raise ValueError(f"{path} is missing the named entity-link search")
        if not {"Focus", "SetValue"}.issubset(set(search.get("on_action") or [])):
            raise ValueError(f"{path} entity-link search is not editable")
    if surface == "documents" and state == "entity_graph":
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
        }
        for required in {
            "Open Product launch",
            "Remove link to Product launch",
            "Open backlink from Rollout checklist",
            "Open backlink in #product",
        }:
            if required not in labels:
                raise ValueError(f"{path} is missing resolved entity action {required!r}")
        exposed_text = " ".join(
            str((node.get("aria") or {}).get(field) or "")
            for node in nodes.values()
            for field in ("label", "description", "value")
        )
        for raw_identifier in {
            "project-launch",
            "document-rollout",
            "channel-product",
            "macro|maya@example.test",
        }:
            if raw_identifier in exposed_text:
                raise ValueError(
                    f"{path} exposes raw entity identifier {raw_identifier!r}"
                )
    if surface == "documents" and state == "forbidden":
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
        }
        if "You don’t have access" not in labels:
            raise ValueError(f"{path} is missing the denied-access state")
        if "No access" not in labels:
            raise ValueError(f"{path} is missing the denied-access capability status")
        if "Check access again" not in labels:
            raise ValueError(f"{path} is missing the access refresh action")
        exposed_text = " ".join(
            str((node.get("aria") or {}).get(field) or "")
            for node in nodes.values()
            for field in ("label", "description", "value")
        )
        for restricted_value in {
            "PHOENIX-741",
            "Confidential launch code",
            "Confidential Owner",
            "confidential-project",
            "confidential-source",
        }:
            if restricted_value in exposed_text:
                raise ValueError(
                    f"{path} exposes denied entity data {restricted_value!r}"
                )
        for forbidden_action in {
            "Share",
            "Link entity",
            "Add comment",
            "Remove link to Confidential project",
        }:
            if forbidden_action in labels:
                raise ValueError(
                    f"{path} exposes denied mutation {forbidden_action!r}"
                )
    if surface == "documents" and state == "realtime_restoring":
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
        }
        if "Checking access…" not in labels:
            raise ValueError(f"{path} is missing the realtime access-check state")
        exposed_text = " ".join(
            str((node.get("aria") or {}).get(field) or "")
            for node in nodes.values()
            for field in ("label", "description", "value")
        )
        for stale_value in {
            "PHOENIX-741",
            "Confidential launch code",
            "Confidential Owner",
            "confidential-project",
            "confidential-source",
        }:
            if stale_value in exposed_text:
                raise ValueError(
                    f"{path} exposes stale entity data while access is reconciling: {stale_value!r}"
                )
    if surface == "canvas" and state == "entity_embed":
        embed_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Embeddable entities"
        ]
        if len(embed_lists) != 1:
            raise ValueError(f"{path} must expose one named embeddable entity list")
        embed_options = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and str((node.get("aria") or {}).get("label") or "").startswith("Embed ")
        ]
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in embed_options
        }
        for required in {
            "Embed Rollout checklist",
            "Embed Ship the native release",
            "Embed Executive launch review",
            "Embed Customer discovery · Northwind",
            "Embed #product",
            "Embed Product launch",
        }:
            if required not in labels:
                raise ValueError(f"{path} is missing typed canvas entity choice {required}")
        if "Embed Launch workflow" in labels:
            raise ValueError(f"{path} canvas entity picker includes source canvas")
        selected = [
            node
            for node in embed_options
            if (node.get("aria") or {}).get("selected") is True
        ]
        if len(selected) != 1:
            raise ValueError(f"{path} must expose exactly one selected canvas entity result")
        inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
        ]
        input_labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in inputs
        }
        if input_labels != {
            "Search workspace entities",
            "Open at subpath (optional)",
        }:
            raise ValueError(f"{path} exposes raw canvas entity transport fields")
        search = next(
            (node.get("aria") or {})
            for node in inputs
            if (node.get("aria") or {}).get("label") == "Search workspace entities"
        )
        if not {"Focus", "SetValue"}.issubset(set(search.get("on_action") or [])):
            raise ValueError(f"{path} canvas entity search is not editable")
    if surface == "settings" and state in {"tag_create", "tag_edit"}:
        color_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Tag colors"
        ]
        if len(color_lists) != 1:
            raise ValueError(f"{path} must expose one named tag color list")
        color_buttons = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and str((node.get("aria") or {}).get("label") or "").endswith(
                " tag color"
            )
        ]
        expected_colors = {
            f"{name} tag color"
            for name in [
                "Red",
                "Tomato",
                "Orange",
                "Amber",
                "Yellow",
                "Green",
                "Teal",
                "Blue",
                "Indigo",
                "Purple",
                "Pink",
                "Gray",
            ]
        }
        labels = {
            str((node.get("aria") or {}).get("label") or "")
            for node in color_buttons
        }
        if labels != expected_colors:
            raise ValueError(f"{path} does not expose Macro's fixed tag palette")
        selected = {
            str((node.get("aria") or {}).get("label") or "")
            for node in color_buttons
            if (node.get("aria") or {}).get("selected") is True
        }
        expected_selected = {
            "Gray tag color" if state == "tag_create" else "Orange tag color"
        }
        if selected != expected_selected:
            raise ValueError(f"{path} does not expose the selected tag color")
        inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
        ]
        if len(inputs) != 1:
            raise ValueError(f"{path} exposes raw tag scope or color fields")
        name_input = inputs[0].get("aria") or {}
        if name_input.get("label") != "Name":
            raise ValueError(f"{path} is missing the named tag input")
        if not {"Focus", "SetValue"}.issubset(
            set(name_input.get("on_action") or [])
        ):
            raise ValueError(f"{path} tag name is not editable")
        expected_name = "" if state == "tag_create" else "Launch"
        if name_input.get("value") != expected_name:
            raise ValueError(f"{path} does not expose the current tag name")
        scope_buttons = {
            str((node.get("aria") or {}).get("label") or ""): (
                node.get("aria") or {}
            ).get("toggled")
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "")
            in {"Personal", "Workspace"}
        }
        expected_scope = (
            {"Personal": "True", "Workspace": "False"}
            if state == "tag_create"
            else {}
        )
        if scope_buttons != expected_scope:
            raise ValueError(f"{path} does not expose the native tag sharing mode")
    if surface == "settings" and state == "property_create":
        inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
        ]
        input_values = {
            str((node.get("aria") or {}).get("label") or ""): (
                node.get("aria") or {}
            ).get("value")
            for node in inputs
        }
        if input_values != {
            "Display name": "Launch stage",
            "Options": "Planned, In progress, Done",
        }:
            raise ValueError(f"{path} exposes raw property-definition transport fields")
        for node in inputs:
            actions = set((node.get("aria") or {}).get("on_action") or [])
            if not {"Focus", "SetValue"}.issubset(actions):
                raise ValueError(f"{path} property-definition input is not editable")
        type_buttons = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "ComboBox"
            and (node.get("aria") or {}).get("label") == "Property type"
        ]
        if len(type_buttons) != 1 or (
            type_buttons[0].get("aria") or {}
        ).get("value") != "select string":
            raise ValueError(f"{path} is missing the native property-type menu")
        toggles = {
            str((node.get("aria") or {}).get("label") or ""): (
                node.get("aria") or {}
            ).get("toggled")
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "")
            in {"Workspace", "Personal", "Single", "Multi"}
        }
        if toggles != {
            "Workspace": "True",
            "Personal": "False",
            "Single": "False",
            "Multi": "True",
        }:
            raise ValueError(f"{path} does not expose native owner and selection modes")
    if surface == "settings" and state == "workspace_edit":
        inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
        ]
        input_values = {
            str((node.get("aria") or {}).get("label") or ""): (
                node.get("aria") or {}
            ).get("value")
            for node in inputs
        }
        if input_values != {"Name": "Product", "Slug": "product"}:
            raise ValueError(f"{path} exposes raw workspace access transport fields")
        for node in inputs:
            if not {"Focus", "SetValue"}.issubset(
                set((node.get("aria") or {}).get("on_action") or [])
            ):
                raise ValueError(f"{path} workspace identity field is not editable")
        link_modes = {
            str((node.get("aria") or {}).get("label") or ""): (
                node.get("aria") or {}
            ).get("toggled")
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "")
            in {"No default", "Workspace", "Public"}
        }
        if link_modes != {
            "No default": "False",
            "Workspace": "True",
            "Public": "False",
        }:
            raise ValueError(f"{path} does not expose native default link sharing modes")
    if surface == "settings" and state == "role_edit":
        if any(
            (node.get("aria") or {}).get("role") == "TextInput"
            for node in nodes.values()
        ):
            raise ValueError(f"{path} exposes a raw workspace role field")
        roles = {
            str((node.get("aria") or {}).get("label") or ""): (
                node.get("aria") or {}
            ).get("toggled")
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "")
            in {"Member", "Admin", "Owner"}
        }
        if roles != {"Member": "False", "Admin": "True", "Owner": "False"}:
            raise ValueError(f"{path} does not expose native workspace role choices")
    if surface == "channels" and state == "thread_target":
        pagination_actions = {
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
        }
        if not {"load older messages", "load newer messages"}.issubset(
            pagination_actions
        ):
            raise ValueError(
                f"{path} does not expose both directions of centered channel pagination"
            )
        target_threads = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "ListItem"
            and "Target thread ·" in str((node.get("aria") or {}).get("label") or "")
        ]
        if len(target_threads) != 1:
            raise ValueError(
                f"{path} must paint exactly one named deep-link target thread"
            )
        target_replies = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "ListItem"
            and "Target reply ·" in str((node.get("aria") or {}).get("label") or "")
        ]
        if len(target_replies) != 1:
            raise ValueError(
                f"{path} must paint exactly one named deep-link target reply"
            )
    if surface == "channels" and state == "invite_acceptance":
        join_actions = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and (node.get("aria") or {}).get("label") == "Join Channel"
        ]
        if len(join_actions) != 1:
            raise ValueError(
                f"{path} must expose exactly one named Join Channel confirmation action"
            )
        all_accessible_text = " ".join(
            str(value)
            for node in nodes.values()
            for value in (node.get("aria") or {}).values()
            if isinstance(value, str)
        )
        if "Confirm that you want to join this private Macro channel." not in all_accessible_text:
            raise ValueError(f"{path} does not explain the channel join confirmation")
        if "fixture-private-invite" in all_accessible_text:
            raise ValueError(f"{path} leaks the opaque channel join code into accessibility text")
    if surface == "channels" and state == "participants_typing":
        button_labels = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
        ]
        if button_labels.count("Copy invite link") != 1:
            raise ValueError(f"{path} does not expose one reusable private invite action")
        if button_labels.count("Message") != 2:
            raise ValueError(f"{path} does not expose DM actions for both other participants")
        typing_statuses = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Status"
        ]
        if "Maya Chen is typing…" not in typing_statuses:
            raise ValueError(f"{path} does not expose the realtime typing state as status")
    if surface == "channels" and state == "rich_composer":
        button_labels = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
        ]
        for required in {"@here", "@Macro", "Attach files…", "Retry"}:
            if button_labels.count(required) != 1:
                raise ValueError(
                    f"{path} must expose exactly one rich-composer action {required!r}"
                )
        attachment_rows = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "ListItem"
            and any(kind in str((node.get("aria") or {}).get("label") or "") for kind in ("Image ", "Video ", "Document "))
        ]
        if len(attachment_rows) != 3:
            raise ValueError(
                f"{path} must expose three typed composer attachments; got {attachment_rows}"
            )
        statuses = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Status"
        ]
        for expected in {
            "launch-preview.png: Ready",
            "desktop-demo.mp4: Uploading…",
            "release-checklist.pdf: Upload interrupted · retry available",
        }:
            if expected not in statuses:
                raise ValueError(f"{path} is missing upload status {expected!r}")
        remove_actions = [label for label in button_labels if label.startswith("Remove ")]
        if len(remove_actions) != 3:
            raise ValueError(f"{path} must expose one remove action per attachment")
    if surface == "channels" and state == "edit_message":
        button_labels = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
        ]
        for required in {
            "@Macro",
            "Attach files…",
            "Remove launch-preview.png",
            "Remove launch-plan.pdf",
            "Save changes",
        }:
            if button_labels.count(required) != 1:
                raise ValueError(
                    f"{path} must expose exactly one rich-edit action {required!r}"
                )
        attachment_rows = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "ListItem"
            and str((node.get("aria") or {}).get("label") or "").endswith(
                ", Ready"
            )
        ]
        if len(attachment_rows) != 2:
            raise ValueError(
                f"{path} must seed both existing attachments into edit mode; got {attachment_rows}"
            )
    if surface == "channels" and state == "reconnecting":
        statuses = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Status"
        ]
        if "Connecting" not in statuses:
            raise ValueError(f"{path} does not expose reconnecting transport status")
        reconnect_guidance = [
            label
            for label in statuses
            if "Pending messages retain their original idempotency keys" in label
        ]
        if len(reconnect_guidance) != 1:
            raise ValueError(f"{path} does not expose one reconnect recovery notice")
        if any("typing" in label.lower() for label in statuses):
            raise ValueError(f"{path} retained stale typing presence while disconnected")
        message_labels = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "ListItem"
        ]
        if sum("Release notes from this Mac will retry after reconnect" in label for label in message_labels) != 1:
            raise ValueError(f"{path} must retain exactly one pending reconnect message")
    if surface == "channels" and state == "populated":
        channel_rows = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
            and str((node.get("aria") or {}).get("label") or "").startswith("#")
        ]
        unread_rows = [label for label in channel_rows if label.endswith(", unread")]
        read_rows = [label for label in channel_rows if label.endswith(", read")]
        if len(unread_rows) != 1 or len(read_rows) != 2:
            raise ValueError(
                f"{path} must expose one unread and two read channel rows; got {channel_rows}"
            )
    if surface == "integrations" and state == "bot_token":
        button_labels = [
            str((node.get("aria") or {}).get("label") or "")
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Button"
        ]
        for required in {"Copy", "Dismiss"}:
            if button_labels.count(required) != 1:
                raise ValueError(
                    f"{path} must expose one named one-time credential action {required!r}"
                )
        all_accessible_text = " ".join(
            str(value)
            for node in nodes.values()
            for value in (node.get("aria") or {}).values()
            if isinstance(value, str)
        )
        for expected in {
            "@release webhook token",
            "x-macro-channel-bot-token",
            "Macro will not return it again",
        }:
            if expected not in all_accessible_text:
                raise ValueError(
                    f"{path} is missing one-time bot credential guidance {expected!r}"
                )
    if surface == "integrations" and state == "webhook_edit":
        toggles = {
            str((node.get("aria") or {}).get("label") or ""): (
                node.get("aria") or {}
            ).get("toggled")
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "")
            in {"Active", "Paused", "Disabled"}
        }
        if toggles != {"Active": "True", "Paused": "False", "Disabled": "False"}:
            raise ValueError(
                f"{path} does not expose native webhook status choices: {toggles}"
            )
        raw_status_inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
            and str((node.get("aria") or {}).get("label") or "").startswith("Status")
        ]
        if raw_status_inputs:
            raise ValueError(f"{path} exposes a raw webhook status text field")
    if surface == "inbox" and state == "populated":
        unread_rows = [
            node
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "").endswith(", unread")
        ]
        if not unread_rows:
            raise ValueError(f"{path} exposes no named unread inbox rows")
        for node in unread_rows:
            aria = node.get("aria") or {}
            shortcut = str(aria.get("keyboard_shortcut") or "")
            description = str(aria.get("description") or "")
            if "Shift+Enter" not in shortcut or "Shift+Enter opens in the split" not in description:
                raise ValueError(
                    f"{path} unread row does not expose the fullscreen/split keyboard contract"
                )
        selection_controls = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "CheckBox"
            and str((node.get("aria") or {}).get("label") or "").startswith("Select ")
        ]
        if not selection_controls:
            raise ValueError(f"{path} exposes no checkbox semantics for inbox selection")
        read_filters = [
            node
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "").startswith(
                "Inbox read filter: Unread"
            )
        ]
        if len(read_filters) != 1:
            raise ValueError(f"{path} must expose the default Unread inbox filter")
    if surface == "search" and state == "populated":
        search_inputs = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TextInput"
            and (node.get("aria") or {}).get("label") == "Search all Macro content"
        ]
        if len(search_inputs) != 1:
            raise ValueError(
                f"{path} must expose exactly one native unified search text input"
            )
        search_input = search_inputs[0].get("aria") or {}
        if not {"Focus", "SetValue"}.issubset(search_input.get("on_action") or []):
            raise ValueError(
                f"{path} native unified search text input must support Focus and SetValue"
            )
        if search_input.get("value") != "launch plan" or not search_input.get(
            "placeholder"
        ):
            raise ValueError(
                f"{path} native unified search text input does not expose its value and placeholder"
            )
        search_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Search results"
        ]
        if len(search_lists) != 1:
            raise ValueError(f"{path} exposes no named search-results list")
        search_rows = [
            node
            for node in nodes.values()
            if "Up and Down move between results"
            in str((node.get("aria") or {}).get("description") or "")
        ]
        if len(search_rows) != 4:
            raise ValueError(
                f"{path} must expose four keyboard-navigable search results"
            )
        if sum(bool((node.get("aria") or {}).get("selected")) for node in search_rows) != 1:
            raise ValueError(f"{path} must expose exactly one selected search result")
        active_result = nodes.get(tree.get("active_descendant_focus")) or {}
        if "Up and Down move between results" not in str(
            (active_result.get("aria") or {}).get("description") or ""
        ):
            raise ValueError(
                f"{path} search-results focus does not report its active descendant"
            )
        positions = []
        for node in search_rows:
            aria = node.get("aria") or {}
            positions.append(aria.get("position_in_set"))
            if aria.get("size_of_set") != 4:
                raise ValueError(
                    f"{path} search result does not expose its set position"
                )
            if "Shift+Enter" not in str(aria.get("keyboard_shortcut") or ""):
                raise ValueError(
                    f"{path} search result does not expose its split-open shortcut"
                )
        if sorted(positions) != [1, 2, 3, 4]:
            raise ValueError(f"{path} search result positions are not complete")
    if surface == "files" and state == "populated":
        folder_trees = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "Tree"
            and (node.get("aria") or {}).get("label") == "Folders"
        ]
        if len(folder_trees) != 1:
            raise ValueError(f"{path} must expose exactly one named Files folder tree")
        folder_rows = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "TreeItem"
            and "Left and Right collapse or expand"
            in str((node.get("aria") or {}).get("description") or "")
        ]
        if len(folder_rows) != 4:
            raise ValueError(
                f"{path} must expose four keyboard-navigable Files folder rows"
            )
        levels = sorted((node.get("aria") or {}).get("level") for node in folder_rows)
        if levels != [1, 1, 2, 2]:
            raise ValueError(f"{path} Files folder levels are incomplete: {levels}")
        for node in folder_rows:
            aria = node.get("aria") or {}
            if not aria.get("position_in_set") or not aria.get("size_of_set"):
                raise ValueError(
                    f"{path} Files folder row does not expose its sibling position"
                )
            shortcut = str(aria.get("keyboard_shortcut") or "")
            if "ArrowLeft" not in shortcut or "ArrowRight" not in shortcut:
                raise ValueError(
                    f"{path} Files folder row does not expose tree navigation shortcuts"
                )
        launch_rows = [
            node
            for node in folder_rows
            if (node.get("aria") or {}).get("label") == "Product launch"
        ]
        if len(launch_rows) != 1 or not (launch_rows[0].get("aria") or {}).get(
            "expanded"
        ):
            raise ValueError(f"{path} Product launch must be an expanded folder row")
        if sum(bool((node.get("aria") or {}).get("selected")) for node in folder_rows) != 1:
            raise ValueError(f"{path} must expose exactly one selected Files folder")
        active_folder = nodes.get(tree.get("active_descendant_focus")) or {}
        if (active_folder.get("aria") or {}).get("label") != "Product launch":
            raise ValueError(
                f"{path} Files folder tree does not report its active descendant"
            )
        disclosures = [
            node
            for node in nodes.values()
            if str((node.get("aria") or {}).get("label") or "").startswith(
                "Collapse Product launch"
            )
        ]
        if len(disclosures) != 1:
            raise ValueError(f"{path} Files folder tree exposes no collapse control")
    if surface in {"documents", "tasks", "agents", "calendar"} and state == "populated":
        shared_rows = [
            node
            for node in nodes.values()
            if "Shift+Enter opens in the split"
            in str((node.get("aria") or {}).get("description") or "")
        ]
        if not shared_rows:
            raise ValueError(f"{path} exposes no shared keyboard-navigable list rows")
        shared_checkboxes = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "CheckBox"
            and str((node.get("aria") or {}).get("label") or "").startswith("Select ")
        ]
        if not shared_checkboxes:
            raise ValueError(f"{path} exposes no shared multi-select checkboxes")
    if surface == "activity" and state == "populated":
        activity_rows = [
            node
            for node in nodes.values()
            if "Up and Down move between notifications"
            in str((node.get("aria") or {}).get("description") or "")
        ]
        if len(activity_rows) != 3:
            raise ValueError(
                f"{path} must expose three keyboard-navigable Activity notifications"
            )
        if sorted(
            (node.get("aria") or {}).get("position_in_set")
            for node in activity_rows
        ) != [1, 2, 3]:
            raise ValueError(f"{path} Activity positions are incomplete")
        activity_checkboxes = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "CheckBox"
            and str((node.get("aria") or {}).get("label") or "").startswith("Select ")
        ]
        if len(activity_checkboxes) != 3:
            raise ValueError(f"{path} Activity does not expose multi-selection checkboxes")
        active_activity = nodes.get(tree.get("active_descendant_focus")) or {}
        if "Up and Down move between notifications" not in str(
            (active_activity.get("aria") or {}).get("description") or ""
        ):
            raise ValueError(f"{path} Activity does not report its active descendant")
    if surface == "reminders" and state == "populated":
        reminder_rows = [
            node
            for node in nodes.values()
            if "Up and Down move between reminders"
            in str((node.get("aria") or {}).get("description") or "")
        ]
        if len(reminder_rows) != 3:
            raise ValueError(f"{path} must expose three keyboard-navigable Reminders")
        if sorted(
            (node.get("aria") or {}).get("position_in_set")
            for node in reminder_rows
        ) != [1, 2, 3]:
            raise ValueError(f"{path} Reminder positions are incomplete")
        reminder_checkboxes = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "CheckBox"
            and str((node.get("aria") or {}).get("label") or "").startswith("Select ")
        ]
        if len(reminder_checkboxes) != 3:
            raise ValueError(f"{path} Reminders do not expose multi-selection checkboxes")
        active_reminder = nodes.get(tree.get("active_descendant_focus")) or {}
        if "Up and Down move between reminders" not in str(
            (active_reminder.get("aria") or {}).get("description") or ""
        ):
            raise ValueError(f"{path} Reminders do not report their active descendant")
    if surface == "crm" and state in {"populated", "list_grouped"}:
        crm_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "CRM customers"
        ]
        if len(crm_lists) != 1:
            raise ValueError(f"{path} must expose exactly one named CRM customer list")
        crm_rows = [
            node
            for node in nodes.values()
            if "Up and Down move between customers"
            in str((node.get("aria") or {}).get("description") or "")
        ]
        if len(crm_rows) != 4:
            raise ValueError(f"{path} must expose four keyboard-navigable CRM records")
        if sorted(
            (node.get("aria") or {}).get("position_in_set") for node in crm_rows
        ) != [1, 2, 3, 4]:
            raise ValueError(f"{path} CRM record positions are incomplete")
        for node in crm_rows:
            aria = node.get("aria") or {}
            if aria.get("size_of_set") != 4:
                raise ValueError(f"{path} CRM record does not expose its set size")
            shortcut = str(aria.get("keyboard_shortcut") or "")
            if "Shift+Enter" not in shortcut or "X" not in shortcut:
                raise ValueError(f"{path} CRM record does not expose keyboard shortcuts")
        crm_checkboxes = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "CheckBox"
            and str((node.get("aria") or {}).get("label") or "").startswith("Select ")
        ]
        if len(crm_checkboxes) != 5:
            raise ValueError(
                f"{path} CRM must expose four record checkboxes and select-all"
            )
        active_crm = nodes.get(tree.get("active_descendant_focus")) or {}
        if "Up and Down move between customers" not in str(
            (active_crm.get("aria") or {}).get("description") or ""
        ):
            raise ValueError(f"{path} CRM does not report its active descendant")
    if surface == "calls" and state in {"populated", "ringing"}:
        call_lists = [
            node
            for node in nodes.values()
            if (node.get("aria") or {}).get("role") == "List"
            and (node.get("aria") or {}).get("label") == "Active calls"
        ]
        if len(call_lists) != 1:
            raise ValueError(f"{path} must expose exactly one named active-call list")
        call_rows = [
            node
            for node in nodes.values()
            if "Escape returns to recordings"
            in str((node.get("aria") or {}).get("description") or "")
        ]
        if len(call_rows) != 1:
            raise ValueError(f"{path} must expose one keyboard-navigable active call")
        call_row = call_rows[0].get("aria") or {}
        if (
            call_row.get("position_in_set") != 1
            or call_row.get("size_of_set") != 1
            or not call_row.get("selected")
        ):
            raise ValueError(f"{path} active call does not expose focus and set position")
        shortcut = str(call_row.get("keyboard_shortcut") or "")
        if "Enter" not in shortcut or "Space" not in shortcut:
            raise ValueError(f"{path} active call does not expose join shortcuts")
        active_call = nodes.get(tree.get("active_descendant_focus")) or {}
        if "Escape returns to recordings" not in str(
            (active_call.get("aria") or {}).get("description") or ""
        ):
            raise ValueError(f"{path} Calls does not report its active descendant")
    return {
        "node_count": len(nodes),
        "tab_stop_count": int(frame["tab_stop_count"]),
        "actionable_node_count": len(actionable),
        "unlabeled_actionable_node_count": 0,
        "technical_actionable_node_count": 0,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path("crates/macro-gpui/PARITY_FIXTURE_MANIFEST.json"),
    )
    parser.add_argument(
        "--runner",
        type=Path,
        help=(
            "explicit prebuilt runner to use instead of building and staging "
            "the current macro_visual_test_runner"
        ),
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("target/macro-gpui/visual-current"),
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=min(4, os.cpu_count() or 1),
        help="parallel renderer processes (default: up to 4)",
    )
    parser.add_argument("--validate-only", action="store_true")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be at least 1")

    runner_was_built = False
    if not args.validate_only:
        if args.runner is None:
            args.runner = build_and_stage_runner()
            runner_was_built = True
        else:
            args.runner = args.runner.resolve()
            if not args.runner.is_file():
                raise FileNotFoundError(f"missing GPUI capture runner {args.runner}")

    manifest = json.loads(args.manifest.read_text())
    baseline = json.loads(BASELINE_PATH.read_text())
    capture = manifest["capture"]
    runner_contract = manifest["gpui_capture_runner"]
    themes = ["dark" if "dark" in theme.lower() else "light" for theme in runner_contract["themes"]]
    surfaces = runner_contract["surfaces"]
    states = runner_contract["states"]
    missing_states = [surface for surface in surfaces if surface not in states]
    if missing_states:
        raise ValueError(f"GPUI capture surfaces have no state matrix: {missing_states}")
    if len(themes) != len(set(themes)):
        raise ValueError("GPUI capture themes must map uniquely to light/dark")

    expected_width = int(capture["viewport"]["width"] * capture["viewport"]["scale"])
    expected_height = int(capture["viewport"]["height"] * capture["viewport"]["scale"])
    args.output_dir.mkdir(parents=True, exist_ok=True)
    specs = [
        (theme, surface, state)
        for theme in themes
        for surface in surfaces
        for state in states[surface]
    ]

    def capture_one(spec: tuple[str, str, str]) -> dict[str, object]:
        theme, surface, state = spec
        output = args.output_dir / f"{surface}-{state}-one-{theme}.png"
        accessibility_output = (
            args.output_dir / f"{surface}-{state}-one-{theme}.a11y.json"
        )
        if not args.validate_only:
            completed = subprocess.run(
                [
                    str(args.runner),
                    "--theme",
                    theme,
                    "--surface",
                    surface,
                    "--state",
                    state,
                    "--output",
                    str(output),
                    "--accessibility-output",
                    str(accessibility_output),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            if completed.returncode != 0:
                detail = (completed.stderr or completed.stdout).strip()
                raise RuntimeError(
                    f"GPUI capture failed for {surface}/{state}/{theme}: {detail}"
                )
        if not output.is_file():
            raise FileNotFoundError(f"missing GPUI capture {output}")
        if not accessibility_output.is_file():
            raise FileNotFoundError(
                f"missing GPUI accessibility capture {accessibility_output}"
            )
        width, height = png_dimensions(output)
        if (width, height) != (expected_width, expected_height):
            raise ValueError(
                f"{output} is {width}x{height}; expected "
                f"{expected_width}x{expected_height}"
            )
        accessibility = validate_accessibility_tree(
            accessibility_output, surface=surface, state=state
        )
        return {
            "surface": surface,
            "state": state,
            "theme": theme,
            "path": output.name,
            "width": width,
            "height": height,
            "bytes": output.stat().st_size,
            "sha256": sha256(output),
            "accessibility": {
                "path": accessibility_output.name,
                "bytes": accessibility_output.stat().st_size,
                "sha256": sha256(accessibility_output),
                **accessibility,
            },
        }

    if args.validate_only or args.jobs == 1:
        captures = [capture_one(spec) for spec in specs]
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
            captures = list(executor.map(capture_one, specs))

    attestation_path = args.output_dir / "candidate-manifest.json"
    runner_attestation = None
    if args.runner is not None:
        runner_attestation = {
            "path": str(args.runner),
            "bytes": args.runner.stat().st_size,
            "sha256": sha256(args.runner),
            "built_by_capture_script": runner_was_built,
        }
    elif args.validate_only and attestation_path.is_file():
        # Validation must not erase the provenance recorded by the render run.
        runner_attestation = json.loads(attestation_path.read_text()).get("runner")

    attestation = {
        "schema": "macro.gpui.visual-candidates.v2",
        "fixture_profile": manifest["fixture_profile"],
        "source_manifest": str(args.manifest),
        "source_manifest_sha256": sha256(args.manifest),
        "source_revisions": baseline["source_revisions"],
        "runner": runner_attestation,
        "expected_capture_count": sum(len(states[surface]) for surface in surfaces)
        * len(themes),
        "capture_count": len(captures),
        "captures": captures,
    }
    if attestation["capture_count"] != attestation["expected_capture_count"]:
        raise ValueError("captured GPUI fixture count does not match the manifest")
    attestation_path.write_text(json.dumps(attestation, indent=2) + "\n")
    print(
        f"Validated {len(captures)} GPUI candidate captures; "
        f"attestation written to {attestation_path}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
