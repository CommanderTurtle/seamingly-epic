"""Native-process and configuration helpers for the ComfyUI nodes."""

from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent


def native_binary() -> Path:
    """Locate the explicitly configured or locally built native engine."""

    executable = "seamingly-epic.exe" if os.name == "nt" else "seamingly-epic"
    configured = os.environ.get("SEAMINGLY_EPIC_BIN")
    candidates = [
        Path(configured).expanduser() if configured else None,
        ROOT / "bin" / executable,
        ROOT / "target" / "release" / executable,
        Path(found) if (found := shutil.which("seamingly-epic")) else None,
    ]
    for candidate in candidates:
        if candidate and candidate.is_file():
            return candidate.resolve()
    raise RuntimeError(
        "Seamingly Epic native engine was not found. Run setup.ps1 on Windows "
        "or ./setup.sh on Linux, then restart ComfyUI."
    )


def run_native(arguments: list[str]) -> str:
    """Run the native engine without a shell and return its UTF-8 stdout."""

    command = [str(native_binary()), *arguments]
    completed = subprocess.run(  # noqa: S603 - executable is resolved from trusted local paths
        command,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise RuntimeError(
            f"Seamingly Epic exited with code {completed.returncode}:\n{detail}"
        )
    return completed.stdout


def correction_config(
    *,
    grid_columns: int,
    grid_rows: int,
    x_seams: str,
    y_seams: str,
    scan_radius: int,
    refine_radius: int,
    sample_stride: int,
    blend_width: int,
    profile_smooth_radius: int,
    strength: float,
    local_strength: float,
    max_gain_stops: float,
    min_confidence: float,
    transfer: str,
    threads: int,
) -> dict[str, Any]:
    """Build the Rust descriptor using the same defaults as the CLI."""

    return {
        "seams": {
            "x": parse_coordinates(x_seams),
            "y": parse_coordinates(y_seams),
            "grid": {"columns": int(grid_columns), "rows": int(grid_rows)},
        },
        "scan_radius": int(scan_radius),
        "refine_radius": int(refine_radius),
        "sample_stride": int(sample_stride),
        "blend_width": int(blend_width),
        "profile_smooth_radius": int(profile_smooth_radius),
        "strength": float(strength),
        "local_strength": float(local_strength),
        "max_gain_stops": float(max_gain_stops),
        "min_confidence": float(min_confidence),
        "transfer": transfer,
        "threads": int(threads),
    }


def cli_settings(config: dict[str, Any]) -> list[str]:
    """Translate a descriptor config to equivalent native CLI flags."""

    seams = config["seams"]
    grid = seams["grid"]
    result = [
        "--grid",
        f"{grid['columns']}x{grid['rows']}",
        "--scan-radius",
        str(config["scan_radius"]),
        "--refine-radius",
        str(config["refine_radius"]),
        "--sample-stride",
        str(config["sample_stride"]),
        "--blend-width",
        str(config["blend_width"]),
        "--profile-smooth-radius",
        str(config["profile_smooth_radius"]),
        "--strength",
        str(config["strength"]),
        "--local-strength",
        str(config["local_strength"]),
        "--max-gain-stops",
        str(config["max_gain_stops"]),
        "--min-confidence",
        str(config["min_confidence"]),
        "--transfer",
        str(config["transfer"]),
        "--threads",
        str(config["threads"]),
    ]
    if seams["x"]:
        result.extend(["--x-seams", ",".join(map(str, seams["x"]))])
    if seams["y"]:
        result.extend(["--y-seams", ",".join(map(str, seams["y"]))])
    return result


def parse_coordinates(value: str) -> list[int]:
    """Parse comma/space-delimited non-negative output-pixel coordinates."""

    if not value.strip():
        return []
    tokens = value.replace(";", ",").replace(" ", ",").split(",")
    coordinates: list[int] = []
    for token in tokens:
        if not token:
            continue
        try:
            coordinate = int(token)
        except ValueError as error:
            raise ValueError(f"Invalid seam coordinate: {token!r}") from error
        if coordinate < 0:
            raise ValueError("Seam coordinates cannot be negative")
        coordinates.append(coordinate)
    return sorted(set(coordinates))


def temporary_directory() -> tempfile.TemporaryDirectory[str]:
    """Use an optional large local scratch disk for float32 transport."""

    root = os.environ.get("SEAMINGLY_EPIC_TEMP")
    if root:
        Path(root).expanduser().mkdir(parents=True, exist_ok=True)
    return tempfile.TemporaryDirectory(
        prefix="seamingly-epic-", dir=str(Path(root).expanduser()) if root else None
    )


def report_mask(
    reports: list[dict[str, Any]],
    batch: int,
    height: int,
    width: int,
    blend_width: int,
):
    """Create a Comfy MASK showing where accepted corrections can act."""

    import torch

    masks = torch.zeros((batch, height, width), dtype=torch.float32)
    if blend_width <= 0:
        return masks
    for batch_index, report in enumerate(reports[:batch]):
        for boundary in report.get("boundaries", []):
            if not boundary.get("accepted", False):
                continue
            coordinate = int(boundary["coordinate"])
            start = int(boundary["segment_start"])
            end = int(boundary["segment_end"])
            confidence = float(boundary.get("confidence", 0.0))
            if boundary["orientation"] == "vertical":
                low = max(0, coordinate - blend_width + 1)
                high = min(width, coordinate + blend_width)
                if high <= low or end <= start:
                    continue
                axis = torch.arange(low, high, dtype=torch.float32)
                fade = 0.5 * (
                    1.0
                    + torch.cos(
                        math.pi
                        * (axis - float(coordinate)).abs()
                        / float(blend_width)
                    )
                )
                region = masks[batch_index, max(0, start) : min(height, end), low:high]
                region.copy_(torch.maximum(region, fade.unsqueeze(0) * confidence))
            else:
                low = max(0, coordinate - blend_width + 1)
                high = min(height, coordinate + blend_width)
                if high <= low or end <= start:
                    continue
                axis = torch.arange(low, high, dtype=torch.float32)
                fade = 0.5 * (
                    1.0
                    + torch.cos(
                        math.pi
                        * (axis - float(coordinate)).abs()
                        / float(blend_width)
                    )
                )
                region = masks[batch_index, low:high, max(0, start) : min(width, end)]
                region.copy_(torch.maximum(region, fade.unsqueeze(1) * confidence))
    return masks


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2), encoding="utf-8")


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def platform_summary() -> str:
    return f"Python {sys.version_info.major}.{sys.version_info.minor}; {native_binary()}"
