#!/usr/bin/env python3
"""Generate COM-494 preserved ATLABASE capability snapshots.

This is intentionally independent of the live NAS mount. It reads the repo-local
fixture JSON files copied from oc-linus artifacts and writes stable JSON
snapshots that prove rich ATLABASE ranges are not silently discarded.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SOURCE_DIR = ROOT / "assets" / "nas-fixture-profiles"
SNAPSHOT_DIR = ROOT / "assets" / "fixture-capability-snapshots"

RICH_KEYS = (
    "colorWheelSetting",
    "goboSetting",
    "prismSetting",
    "panSetting",
    "tiltSetting",
    "range",
    "staticValue",
    "toggleSetting",
    "smoothing",
    "invert",
)


def attr_name(attr: dict[str, Any]) -> str:
    return str(attr.get("name") or attr.get("type") or "Unnamed")


def detail_kind(attr: dict[str, Any]) -> str | None:
    typ = str(attr.get("type") or "").upper()
    if "colorWheelSetting" in attr:
        return "color_wheel"
    if "goboSetting" in attr:
        return "gobo" if typ == "GOBO" else "macro"
    if "prismSetting" in attr:
        return "prism"
    if "panSetting" in attr or "tiltSetting" in attr:
        return "movement"
    if "range" in attr or "smoothing" in attr or "invert" in attr:
        return "range"
    if "staticValue" in attr:
        return "static"
    if "toggleSetting" in attr:
        return "toggle"
    return None


def slot_count(attr: dict[str, Any]) -> int:
    for key in ("colorWheelSetting", "goboSetting", "prismSetting"):
        value = attr.get(key)
        if isinstance(value, dict):
            items = value.get("colors") or value.get("steps") or []
            return len(items)
    return 0


def snapshot_for(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text())
    profile = data["fixtureProfile"]
    out = {
        "source_file": path.name,
        "fixture_profile_id": profile.get("id"),
        "manufacturer": profile.get("manufacturer"),
        "name": profile.get("name"),
        "type": data.get("type"),
        "version": data.get("version"),
        "modes": [],
    }
    for mode in profile.get("mode", []):
        mode_out = {
            "name": mode.get("name"),
            "channel_count": len(mode.get("attribute", [])),
            "channels": [],
        }
        for index, attr in enumerate(mode.get("attribute", []), start=1):
            rich = {key: attr[key] for key in RICH_KEYS if key in attr}
            mode_out["channels"].append(
                {
                    "channel_offset": index,
                    "name": attr_name(attr),
                    "type": attr.get("type"),
                    "detail_kind": detail_kind(attr),
                    "slot_count": slot_count(attr),
                    "rich_keys": sorted(rich),
                    "source_attribute": attr,
                }
            )
        out["modes"].append(mode_out)
    return out


def main() -> int:
    files = sorted(SOURCE_DIR.glob("*.json"))
    if len(files) != 7:
        raise SystemExit(f"expected 7 NAS fixture profiles in {SOURCE_DIR}, found {len(files)}")
    SNAPSHOT_DIR.mkdir(parents=True, exist_ok=True)
    for source in files:
        snapshot = snapshot_for(source)
        target = SNAPSHOT_DIR / source.name
        target.write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
        print(f"wrote {target.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
