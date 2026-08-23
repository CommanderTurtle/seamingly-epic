"""ComfyUI nodes for native seam normalization and tutorial-style repair."""

from __future__ import annotations

import json
import math
import re
import uuid
from pathlib import Path
from typing import Any

import numpy as np
import torch
import torch.nn.functional as functional

try:
    import folder_paths
except ImportError:  # Allows source inspection outside a ComfyUI checkout.
    folder_paths = None

from .runtime import (
    cli_settings,
    correction_config,
    load_json,
    parse_coordinates,
    report_mask,
    run_native,
    temporary_directory,
    write_json,
)

CATEGORY = "image/seamingly epic"


def _settings_schema() -> dict[str, tuple[Any, ...]]:
    return {
        "grid_columns": ("INT", {"default": 2, "min": 1, "max": 32}),
        "grid_rows": ("INT", {"default": 2, "min": 1, "max": 32}),
        "x_seams": (
            "STRING",
            {"default": "", "multiline": False, "tooltip": "Optional output-pixel X coordinates, e.g. 4096"},
        ),
        "y_seams": (
            "STRING",
            {"default": "", "multiline": False, "tooltip": "Optional output-pixel Y coordinates, e.g. 4096"},
        ),
        "scan_radius": ("INT", {"default": 8, "min": 1, "max": 256}),
        "refine_radius": ("INT", {"default": 0, "min": 0, "max": 64}),
        "sample_stride": ("INT", {"default": 1, "min": 1, "max": 128}),
        "blend_width": ("INT", {"default": 192, "min": 0, "max": 4096}),
        "profile_smooth_radius": (
            "INT",
            {"default": 96, "min": 0, "max": 4096},
        ),
        "strength": ("FLOAT", {"default": 1.0, "min": 0.0, "max": 2.0, "step": 0.01}),
        "local_strength": (
            "FLOAT",
            {"default": 1.0, "min": 0.0, "max": 2.0, "step": 0.01},
        ),
        "max_gain_stops": (
            "FLOAT",
            {"default": 0.75, "min": 0.0, "max": 4.0, "step": 0.01},
        ),
        "min_confidence": (
            "FLOAT",
            {"default": 0.18, "min": 0.0, "max": 1.0, "step": 0.01},
        ),
        "transfer": (["srgb", "linear"], {"default": "srgb"}),
        "threads": ("INT", {"default": 0, "min": 0, "max": 256}),
    }


def _config_from_arguments(arguments: dict[str, Any]) -> dict[str, Any]:
    keys = _settings_schema().keys()
    return correction_config(**{key: arguments[key] for key in keys})


class SeaminglyEpicImage:
    """Correct an in-memory Comfy IMAGE through a float32 native transport."""

    @classmethod
    def INPUT_TYPES(cls):
        return {"required": {"image": ("IMAGE",), **_settings_schema()}}

    RETURN_TYPES = ("IMAGE", "MASK", "STRING")
    RETURN_NAMES = ("corrected_image", "correction_area", "report_json")
    FUNCTION = "correct"
    CATEGORY = CATEGORY
    DESCRIPTION = (
        "Automatic exposure/white-balance seam correction. Uses float32 transport, "
        "does not blur or resample source detail, and returns the full confidence report."
    )

    def correct(self, image: torch.Tensor, **arguments):
        if image.ndim != 4 or image.shape[-1] not in (3, 4):
            raise ValueError("IMAGE must have shape [B,H,W,3] or [B,H,W,4]")
        batch, height, width, channels = map(int, image.shape)
        config = _config_from_arguments(arguments)

        with temporary_directory() as directory:
            scratch = Path(directory)
            input_path = scratch / "input.f32"
            output_path = scratch / "output.f32"
            report_path = scratch / "report.json"
            descriptor_path = scratch / "descriptor.json"
            pixels = (
                image.detach()
                .to(device="cpu", dtype=torch.float32)
                .contiguous()
                .numpy()
                .astype("<f4", copy=False)
            )
            pixels.tofile(input_path)
            write_json(
                descriptor_path,
                {
                    "input": str(input_path),
                    "output": str(output_path),
                    "width": width,
                    "height": height,
                    "batch": batch,
                    "channels": channels,
                    "config": config,
                    "report": str(report_path),
                },
            )
            run_native(["raw-f32", str(descriptor_path)])
            reports = load_json(report_path)
            corrected = np.fromfile(output_path, dtype="<f4")
            expected = batch * height * width * channels
            if corrected.size != expected:
                raise RuntimeError(
                    f"Native output has {corrected.size} samples; expected {expected}"
                )
            corrected_tensor = torch.from_numpy(
                corrected.reshape((batch, height, width, channels))
            )
            area = report_mask(
                reports,
                batch=batch,
                height=height,
                width=width,
                blend_width=int(config["blend_width"]),
            )
            return corrected_tensor, area, json.dumps(reports, indent=2)


class SeaminglyEpicFile:
    """Stream a PNG through the native engine without creating a Comfy IMAGE."""

    @classmethod
    def INPUT_TYPES(cls):
        if folder_paths is None:
            images: list[str] = []
        else:
            input_dir = Path(folder_paths.get_input_directory())
            images = sorted(
                str(path.relative_to(input_dir)).replace("\\", "/")
                for path in input_dir.rglob("*.png")
                if path.is_file()
            )
        return {
            "required": {
                "image": (images, {"image_upload": True}),
                "output_prefix": ("STRING", {"default": "seamingly_epic"}),
                **_settings_schema(),
            }
        }

    RETURN_TYPES = ("STRING", "STRING")
    RETURN_NAMES = ("output_path", "report_json")
    OUTPUT_NODE = True
    FUNCTION = "correct"
    CATEGORY = CATEGORY
    DESCRIPTION = (
        "Bounded-memory RGB24/RGBA32/RGB48/RGBA64 PNG path for 8K and 16K "
        "images. The image never becomes a Comfy tensor; recognized PNG "
        "metadata and alpha are preserved."
    )

    def correct(self, image: str, output_prefix: str, **arguments):
        if folder_paths is None:
            raise RuntimeError("This node must run inside ComfyUI")
        source = Path(folder_paths.get_annotated_filepath(image)).resolve()
        if source.suffix.lower() != ".png":
            raise ValueError("The streaming node accepts PNG files only")
        if not source.is_file():
            raise FileNotFoundError(source)
        output_dir = Path(folder_paths.get_output_directory()).resolve()
        output_dir.mkdir(parents=True, exist_ok=True)
        safe_prefix = re.sub(r"[^A-Za-z0-9_.-]+", "_", output_prefix).strip("._")
        safe_prefix = safe_prefix or "seamingly_epic"
        filename = f"{safe_prefix}_{uuid.uuid4().hex[:10]}.png"
        destination = output_dir / filename
        config = _config_from_arguments(arguments)
        stdout = run_native(
            [
                "correct",
                str(source),
                str(destination),
                *cli_settings(config),
            ]
        )
        report = json.loads(stdout)
        return {
            "ui": {
                "images": [
                    {"filename": filename, "subfolder": "", "type": "output"}
                ]
            },
            "result": (str(destination), json.dumps(report, indent=2)),
        }


class SeaminglyEpicReferenceRepair:
    """Condense the tutorial's mask-grow-color-correct-composite workflow."""

    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "refined": ("IMAGE",),
                "reference": ("IMAGE",),
                "mask_mode": (
                    ["provided_or_grid", "provided_plus_grid", "grid_only", "provided_only"],
                    {"default": "provided_or_grid"},
                ),
                "grid_columns": ("INT", {"default": 2, "min": 1, "max": 32}),
                "grid_rows": ("INT", {"default": 2, "min": 1, "max": 32}),
                "x_seams": ("STRING", {"default": ""}),
                "y_seams": ("STRING", {"default": ""}),
                "seam_mask_half_width": (
                    "INT",
                    {"default": 24, "min": 1, "max": 2048},
                ),
                "grow_pixels": ("INT", {"default": 32, "min": 0, "max": 2048}),
                "feather_pixels": (
                    "INT",
                    {"default": 48, "min": 0, "max": 2048},
                ),
                "reference_blur": ("INT", {"default": 0, "min": 0, "max": 512}),
                "exposure_stops": (
                    "FLOAT",
                    {"default": 0.0, "min": -4.0, "max": 4.0, "step": 0.01},
                ),
                "saturation": (
                    "FLOAT",
                    {"default": 1.0, "min": 0.0, "max": 4.0, "step": 0.01},
                ),
                "temperature": (
                    "FLOAT",
                    {"default": 0.0, "min": -1.0, "max": 1.0, "step": 0.01},
                ),
            },
            "optional": {"mask": ("MASK",)},
        }

    RETURN_TYPES = ("IMAGE", "MASK")
    RETURN_NAMES = ("repaired_image", "composite_mask")
    FUNCTION = "repair"
    CATEGORY = CATEGORY
    DESCRIPTION = (
        "One-node version of Rob Adams' SeamFix 2.1 workflow: resize the original "
        "reference, grow/feather a painted or grid mask, color-adjust, and composite. "
        "Use this for semantic artifacts that a photometric correction cannot repair."
    )

    def repair(
        self,
        refined: torch.Tensor,
        reference: torch.Tensor,
        mask_mode: str,
        grid_columns: int,
        grid_rows: int,
        x_seams: str,
        y_seams: str,
        seam_mask_half_width: int,
        grow_pixels: int,
        feather_pixels: int,
        reference_blur: int,
        exposure_stops: float,
        saturation: float,
        temperature: float,
        mask: torch.Tensor | None = None,
    ):
        _validate_image(refined, "refined")
        _validate_image(reference, "reference")
        batch, height, width, channels = map(int, refined.shape)
        reference = _match_batch(reference, batch, "reference")
        reference_chw = reference[..., :3].permute(0, 3, 1, 2).float()
        if reference_chw.shape[-2:] != (height, width):
            reference_chw = functional.interpolate(
                reference_chw,
                size=(height, width),
                mode="bicubic",
                align_corners=False,
                antialias=True,
            )
        if reference_blur > 0:
            reference_chw = _box_blur(reference_chw, reference_blur, passes=3)
        reference_rgb = _color_adjust(
            reference_chw.permute(0, 2, 3, 1),
            exposure_stops,
            saturation,
            temperature,
        )

        grid_mask = _grid_mask(
            batch,
            height,
            width,
            grid_columns,
            grid_rows,
            x_seams,
            y_seams,
            seam_mask_half_width,
            refined.device,
        )
        supplied = _prepare_mask(mask, batch, height, width, refined.device)
        composite_mask = _select_mask(mask_mode, supplied, grid_mask)
        if grow_pixels > 0:
            composite_mask = _dilate(composite_mask, grow_pixels)
        if feather_pixels > 0:
            composite_mask = _box_blur(
                composite_mask.unsqueeze(1), feather_pixels, passes=3
            ).squeeze(1)
        composite_mask = composite_mask.clamp(0.0, 1.0)

        base_rgb = refined[..., :3].float()
        alpha = composite_mask.unsqueeze(-1).to(base_rgb.device)
        output_rgb = base_rgb * (1.0 - alpha) + reference_rgb.to(base_rgb.device) * alpha
        output_rgb = output_rgb.clamp(0.0, 1.0)
        if channels > 3:
            output = torch.cat((output_rgb, refined[..., 3:]), dim=-1)
        else:
            output = output_rgb
        return output, composite_mask


def _validate_image(image: torch.Tensor, name: str) -> None:
    if image.ndim != 4 or image.shape[-1] < 3:
        raise ValueError(f"{name} must have shape [B,H,W,C] with at least 3 channels")


def _match_batch(image: torch.Tensor, batch: int, name: str) -> torch.Tensor:
    if image.shape[0] == batch:
        return image
    if image.shape[0] == 1:
        return image.repeat(batch, 1, 1, 1)
    raise ValueError(f"{name} batch must be 1 or match the refined image batch")


def _grid_mask(
    batch: int,
    height: int,
    width: int,
    columns: int,
    rows: int,
    x_text: str,
    y_text: str,
    half_width: int,
    device: torch.device,
) -> torch.Tensor:
    x_coordinates = parse_coordinates(x_text) or [
        (width * index + columns // 2) // columns for index in range(1, columns)
    ]
    y_coordinates = parse_coordinates(y_text) or [
        (height * index + rows // 2) // rows for index in range(1, rows)
    ]
    result = torch.zeros((batch, height, width), dtype=torch.float32, device=device)
    for coordinate in x_coordinates:
        low = max(0, coordinate - half_width)
        high = min(width, coordinate + half_width + 1)
        if high > low:
            result[:, :, low:high] = 1.0
    for coordinate in y_coordinates:
        low = max(0, coordinate - half_width)
        high = min(height, coordinate + half_width + 1)
        if high > low:
            result[:, low:high, :] = 1.0
    return result


def _prepare_mask(
    mask: torch.Tensor | None,
    batch: int,
    height: int,
    width: int,
    device: torch.device,
) -> torch.Tensor | None:
    if mask is None:
        return None
    if mask.ndim == 2:
        mask = mask.unsqueeze(0)
    if mask.ndim != 3:
        raise ValueError("MASK must have shape [H,W] or [B,H,W]")
    if mask.shape[0] == 1 and batch > 1:
        mask = mask.repeat(batch, 1, 1)
    elif mask.shape[0] != batch:
        raise ValueError("MASK batch must be 1 or match the refined image batch")
    mask = mask.to(device=device, dtype=torch.float32).unsqueeze(1)
    if mask.shape[-2:] != (height, width):
        mask = functional.interpolate(
            mask, size=(height, width), mode="bilinear", align_corners=False
        )
    return mask.squeeze(1).clamp(0.0, 1.0)


def _select_mask(
    mode: str, supplied: torch.Tensor | None, grid: torch.Tensor
) -> torch.Tensor:
    if mode == "grid_only":
        return grid
    if mode == "provided_only":
        if supplied is None:
            raise ValueError("mask_mode is provided_only but no MASK is connected")
        return supplied
    if mode == "provided_plus_grid":
        return grid if supplied is None else torch.maximum(supplied, grid)
    return grid if supplied is None else supplied


def _dilate(mask: torch.Tensor, radius: int) -> torch.Tensor:
    value = mask.unsqueeze(1)
    kernel = 2 * radius + 1
    value = functional.max_pool2d(value, (1, kernel), stride=1, padding=(0, radius))
    value = functional.max_pool2d(value, (kernel, 1), stride=1, padding=(radius, 0))
    return value.squeeze(1)


def _box_blur(value: torch.Tensor, radius: int, passes: int) -> torch.Tensor:
    if radius <= 0:
        return value
    kernel = 2 * radius + 1
    result = value
    for _ in range(passes):
        # Prefix sums make each broad pass O(pixels), independent of radius.
        horizontal = functional.pad(result, (radius, radius, 0, 0), mode="replicate")
        horizontal = functional.pad(horizontal.cumsum(dim=-1), (1, 0, 0, 0))
        result = (horizontal[..., kernel:] - horizontal[..., :-kernel]) / kernel
        vertical = functional.pad(result, (0, 0, radius, radius), mode="replicate")
        vertical = functional.pad(vertical.cumsum(dim=-2), (0, 0, 1, 0))
        result = (vertical[..., kernel:, :] - vertical[..., :-kernel, :]) / kernel
    return result


def _color_adjust(
    image: torch.Tensor,
    exposure_stops: float,
    saturation: float,
    temperature: float,
) -> torch.Tensor:
    gain = 2.0**float(exposure_stops)
    warmth = 2.0 ** (float(temperature) * 0.25)
    balance = image.new_tensor([warmth, 1.0, 1.0 / warmth])
    adjusted = image * gain * balance
    luminance = (
        adjusted[..., 0:1] * 0.2126
        + adjusted[..., 1:2] * 0.7152
        + adjusted[..., 2:3] * 0.0722
    )
    adjusted = luminance + (adjusted - luminance) * float(saturation)
    return adjusted.clamp(0.0, 1.0)


NODE_CLASS_MAPPINGS = {
    "SeaminglyEpicImage": SeaminglyEpicImage,
    "SeaminglyEpicFile": SeaminglyEpicFile,
    "SeaminglyEpicReferenceRepair": SeaminglyEpicReferenceRepair,
}

NODE_DISPLAY_NAME_MAPPINGS = {
    "SeaminglyEpicImage": "Seamingly Epic — Native IMAGE",
    "SeaminglyEpicFile": "Seamingly Epic — Streaming PNG",
    "SeaminglyEpicReferenceRepair": "Seamingly Epic — Reference Repair",
}
