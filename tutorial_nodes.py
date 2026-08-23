"""Self-contained compatibility nodes for the original SeamFix 2.1 workflow.

The workflow published with Rob Adams' tutorial used one node each from
MaraScott, WAS Node Suite, Impact Pack, and Art Venture.  This module keeps the
serialized node type names used by that workflow while carrying only the
small, relevant implementations.  KJNodes' ``GrowMaskWithBlur`` intentionally
remains an external node because it is the one permitted workflow dependency.
"""

from __future__ import annotations

import math
from pathlib import Path
from typing import Any

import numpy as np
import torch
from PIL import Image, ImageEnhance, ImageOps

try:
    import folder_paths
    import nodes as comfy_nodes
except ImportError:  # Keep source inspection and static checking independent of ComfyUI.
    folder_paths = None
    comfy_nodes = None


CATEGORY = "image/seamingly epic/tutorial compatibility"


def _require_comfy() -> None:
    if folder_paths is None or comfy_nodes is None:
        raise RuntimeError("This compatibility node must run inside ComfyUI")


def _tensor_to_pil(image: torch.Tensor) -> Image.Image:
    array = (
        image.detach()
        .to(device="cpu", dtype=torch.float32)
        .clamp(0.0, 1.0)
        .mul(255.0)
        .to(torch.uint8)
        .numpy()
    )
    if array.shape[-1] == 1:
        return Image.fromarray(array[..., 0], mode="L")
    if array.shape[-1] == 4:
        return Image.fromarray(array, mode="RGBA")
    return Image.fromarray(array[..., :3], mode="RGB")


def _pil_to_tensor(image: Image.Image) -> torch.Tensor:
    array = np.asarray(image).astype(np.float32) / 255.0
    if array.ndim == 2:
        array = array[..., None]
    return torch.from_numpy(array.copy()).unsqueeze(0)


class SeamFixImageResize:
    """The tutorial's WAS ``Image Resize`` node, without WAS' unrelated suite."""

    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "image": ("IMAGE",),
                "mode": (["rescale", "resize"],),
                "supersample": (["true", "false"],),
                "resampling": (["lanczos", "nearest", "bilinear", "bicubic"],),
                "rescale_factor": (
                    "FLOAT",
                    {"default": 2.0, "min": 0.01, "max": 16.0, "step": 0.01},
                ),
                "resize_width": (
                    "INT",
                    {"default": 1024, "min": 1, "max": 48000, "step": 1},
                ),
                "resize_height": (
                    "INT",
                    {"default": 1536, "min": 1, "max": 48000, "step": 1},
                ),
            }
        }

    RETURN_TYPES = ("IMAGE",)
    FUNCTION = "image_rescale"
    CATEGORY = CATEGORY
    DESCRIPTION = (
        "Focused compatibility implementation of the Image Resize nodes in "
        "SeamFix 2.1. Its widgets and 8x supersampling behavior match the "
        "original workflow."
    )

    def image_rescale(
        self,
        image: torch.Tensor,
        mode: str = "rescale",
        supersample: str = "true",
        resampling: str = "lanczos",
        rescale_factor: float = 2.0,
        resize_width: int = 1024,
        resize_height: int = 1536,
    ):
        filters = {
            "nearest": Image.Resampling.NEAREST,
            "bilinear": Image.Resampling.BILINEAR,
            "bicubic": Image.Resampling.BICUBIC,
            "lanczos": Image.Resampling.LANCZOS,
        }
        resample = filters[resampling]
        outputs: list[torch.Tensor] = []
        for sample in image:
            source = _tensor_to_pil(sample)
            if mode == "rescale":
                width = max(1, int(source.width * float(rescale_factor)))
                height = max(1, int(source.height * float(rescale_factor)))
            else:
                width = int(resize_width)
                height = int(resize_height)
                width += (-width) % 8
                height += (-height) % 8
            if supersample == "true":
                source = source.resize((width * 8, height * 8), resample=resample)
            outputs.append(_pil_to_tensor(source.resize((width, height), resample=resample)))
        return (torch.cat(outputs, dim=0),)


def _rgb_to_hsv(rgb: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor]:
    maximum, maximum_index = rgb.max(dim=-1)
    minimum = rgb.min(dim=-1).values
    delta = maximum - minimum
    saturation = torch.where(maximum > 0, delta / maximum.clamp_min(1.0e-12), 0.0)
    hue = torch.zeros_like(maximum)
    nonzero = delta > 1.0e-12
    red = ((rgb[..., 1] - rgb[..., 2]) / delta.clamp_min(1.0e-12)) % 6.0
    green = (rgb[..., 2] - rgb[..., 0]) / delta.clamp_min(1.0e-12) + 2.0
    blue = (rgb[..., 0] - rgb[..., 1]) / delta.clamp_min(1.0e-12) + 4.0
    hue = torch.where(maximum_index == 0, red, hue)
    hue = torch.where(maximum_index == 1, green, hue)
    hue = torch.where(maximum_index == 2, blue, hue)
    hue = torch.where(nonzero, hue / 6.0, 0.0)
    return hue, saturation, maximum


def _hsv_to_rgb(hue: torch.Tensor, saturation: torch.Tensor, value: torch.Tensor) -> torch.Tensor:
    sector = torch.floor((hue % 1.0) * 6.0).to(torch.int64)
    fraction = (hue % 1.0) * 6.0 - sector.to(hue.dtype)
    p = value * (1.0 - saturation)
    q = value * (1.0 - fraction * saturation)
    t = value * (1.0 - (1.0 - fraction) * saturation)
    choices = torch.stack(
        (
            torch.stack((value, t, p), dim=-1),
            torch.stack((q, value, p), dim=-1),
            torch.stack((p, value, t), dim=-1),
            torch.stack((p, q, value), dim=-1),
            torch.stack((t, p, value), dim=-1),
            torch.stack((value, p, q), dim=-1),
        ),
        dim=-2,
    )
    index = (sector % 6).unsqueeze(-1).unsqueeze(-1).expand(*sector.shape, 1, 3)
    return choices.gather(-2, index).squeeze(-2)


class SeamFixColorCorrect:
    """Art Venture-compatible color correction with no OpenCV dependency."""

    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "image": ("IMAGE",),
                "temperature": (
                    "FLOAT",
                    {"default": 0.0, "min": -100.0, "max": 100.0, "step": 5.0},
                ),
                "hue": (
                    "FLOAT",
                    {"default": 0.0, "min": -90.0, "max": 90.0, "step": 5.0},
                ),
                "brightness": (
                    "FLOAT",
                    {"default": 0.0, "min": -100.0, "max": 100.0, "step": 5.0},
                ),
                "contrast": (
                    "FLOAT",
                    {"default": 0.0, "min": -100.0, "max": 100.0, "step": 5.0},
                ),
                "saturation": (
                    "FLOAT",
                    {"default": 0.0, "min": -100.0, "max": 100.0, "step": 5.0},
                ),
                "gamma": (
                    "FLOAT",
                    {"default": 1.0, "min": 0.2, "max": 2.2, "step": 0.1},
                ),
            }
        }

    RETURN_TYPES = ("IMAGE",)
    FUNCTION = "color_correct"
    CATEGORY = CATEGORY
    DESCRIPTION = (
        "Focused Art Venture ColorCorrect compatibility for the tutorial's "
        "temperature, hue, brightness, contrast, saturation, and gamma dials."
    )

    def color_correct(
        self,
        image: torch.Tensor,
        temperature: float,
        hue: float,
        brightness: float,
        contrast: float,
        saturation: float,
        gamma: float,
    ):
        device = image.device
        dtype = image.dtype
        corrected: list[torch.Tensor] = []
        for sample in image:
            pil = _tensor_to_pil(sample[..., :3])
            pil = ImageEnhance.Brightness(pil).enhance(1.0 + brightness / 100.0)
            pil = ImageEnhance.Contrast(pil).enhance(1.0 + contrast / 100.0)
            rgb = _pil_to_tensor(pil)[0]
            warmth = temperature / 100.0
            if warmth > 0:
                rgb[..., 0] *= 1.0 + warmth
                rgb[..., 1] *= 1.0 + warmth * 0.4
            elif warmth < 0:
                rgb[..., 2] *= 1.0 - warmth
            rgb = rgb.clamp(0.0, 1.0).pow(float(gamma))

            h, s, v = _rgb_to_hsv(rgb)
            # OpenCV's HLS saturation adjustment is equivalent to preserving
            # lightness while scaling chroma.  Reconstruct HSV after applying it.
            maximum = v
            minimum = maximum * (1.0 - s)
            lightness = (maximum + minimum) * 0.5
            hls_s = torch.where(
                (maximum - minimum) > 1.0e-12,
                (maximum - minimum)
                / (1.0 - (2.0 * lightness - 1.0).abs()).clamp_min(1.0e-12),
                0.0,
            )
            hls_s = (hls_s * (1.0 + saturation / 100.0)).clamp(0.0, 1.0)
            new_v = lightness + hls_s * torch.minimum(lightness, 1.0 - lightness)
            new_s = torch.where(
                new_v > 1.0e-12,
                2.0 * (1.0 - lightness / new_v.clamp_min(1.0e-12)),
                0.0,
            ).clamp(0.0, 1.0)
            h = (h + hue / 360.0) % 1.0
            rgb = _hsv_to_rgb(h, new_s, new_v).clamp(0.0, 1.0)
            # Match the original node's final uint8 round-trip.
            rgb = rgb.mul(255.0).to(torch.uint8).to(torch.float32).div(255.0)
            if sample.shape[-1] > 3:
                rgb = torch.cat((rgb, sample[..., 3:].to(device="cpu")), dim=-1)
            corrected.append(rgb.unsqueeze(0))
        return (torch.cat(corrected, dim=0).to(device=device, dtype=dtype),)


class SeamFixPreviewBridge:
    """A native Comfy mask-editor bridge for the tutorial's PreviewBridge nodes."""

    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "images": ("IMAGE",),
                "image": ("STRING", {"default": ""}),
            },
            "hidden": {"unique_id": "UNIQUE_ID"},
        }

    RETURN_TYPES = ("IMAGE", "MASK")
    RETURN_NAMES = ("image", "painted_mask")
    FUNCTION = "bridge"
    OUTPUT_NODE = True
    CATEGORY = CATEGORY
    DESCRIPTION = (
        "Run once, right-click its preview, choose Open in Mask Editor, paint the "
        "correction area, save to the node, then queue again. The current ComfyUI "
        "mask editor stores the painted mask in this node's image widget."
    )

    def bridge(self, images: torch.Tensor, image: str, unique_id: str):
        _require_comfy()
        if image and not image.startswith("$"):
            try:
                path = Path(folder_paths.get_annotated_filepath(image))
                if path.is_file():
                    with Image.open(path) as opened:
                        opened = ImageOps.exif_transpose(opened)
                        rgb = _pil_to_tensor(opened.convert("RGB"))
                        if "A" in opened.getbands():
                            alpha = np.asarray(opened.getchannel("A")).astype(np.float32) / 255.0
                            mask = 1.0 - torch.from_numpy(alpha.copy()).unsqueeze(0)
                        else:
                            mask = torch.zeros(
                                (rgb.shape[0], rgb.shape[1], rgb.shape[2]),
                                dtype=torch.float32,
                            )
                    item = _ui_item_from_annotated(image, path)
                    return {"ui": {"images": [item]}, "result": (rgb, mask)}
            except (OSError, ValueError):
                # A stale clipspace name simply starts a fresh editable preview.
                pass

        preview = comfy_nodes.PreviewImage().save_images(
            images,
            filename_prefix=f"SeaminglyEpic/MaskBridge-{unique_id}",
        )
        mask = torch.zeros(
            (images.shape[0], images.shape[1], images.shape[2]),
            dtype=torch.float32,
            device=images.device,
        )
        return {"ui": preview["ui"], "result": (images, mask)}


def _ui_item_from_annotated(value: str, path: Path) -> dict[str, str]:
    suffix = "input"
    lowered = value.lower()
    if lowered.endswith("[output]"):
        suffix = "output"
    elif lowered.endswith("[temp]"):
        suffix = "temp"
    subfolder = "clipspace" if "clipspace" in path.parts else ""
    return {"filename": path.name, "subfolder": subfolder, "type": suffix}


def _grid_specs(width: int, height: int) -> list[tuple[int, int, int, int, int]]:
    width_unit = width // 16
    height_unit = height // 16
    tile_width = width_unit * 6
    tile_height = height_unit * 6
    order = (0, 2, 1)
    return [
        (
            column * len(order) + row,
            row * (tile_width - width_unit),
            column * (tile_height - height_unit),
            tile_width,
            tile_height,
        )
        for column in order
        for row in order
    ]


def _edge_feather(
    length: int, feather: int, *, leading: bool, trailing: bool, device: torch.device
) -> torch.Tensor:
    weights = torch.ones(length, dtype=torch.float32, device=device)
    extent = min(max(0, int(feather)), length // 2)
    if extent == 0:
        return weights
    ramp = torch.arange(1, extent + 1, dtype=torch.float32, device=device) / extent
    if leading:
        weights[:extent] = ramp
    if trailing:
        weights[-extent:] = ramp.flip(0)
    return weights


def _paste(
    destination: torch.Tensor,
    source: torch.Tensor,
    x: int,
    y: int,
    mask: torch.Tensor | None = None,
) -> None:
    height = min(source.shape[1], destination.shape[1] - y)
    width = min(source.shape[2], destination.shape[2] - x)
    if height <= 0 or width <= 0:
        return
    source_slice = source[:, :height, :width, :]
    target = destination[:, y : y + height, x : x + width, :]
    if mask is None:
        target.copy_(source_slice)
    else:
        alpha = mask[:height, :width].to(device=target.device, dtype=target.dtype)[None, ..., None]
        target.mul_(1.0 - alpha).add_(source_slice * alpha)


def _rebuild_nine_tiles(
    tiles: list[torch.Tensor], output_shape: tuple[int, int], feather: int
) -> torch.Tensor:
    if len(tiles) != 9:
        raise ValueError("McBoaty compatibility requires exactly nine overlap tiles")
    height, width = output_shape
    batch, _, _, channels = tiles[0].shape
    specs = _grid_specs(width, height)
    rows: list[torch.Tensor] = []
    for group in range(3):
        left, right, middle = tiles[group * 3 : group * 3 + 3]
        row = torch.zeros(
            (batch, left.shape[1], width, channels),
            dtype=left.dtype,
            device=left.device,
        )
        _paste(row, left, specs[group * 3][1], 0)
        _paste(row, right, specs[group * 3 + 1][1], 0)
        horizontal = _edge_feather(
            middle.shape[2], feather, leading=True, trailing=True, device=middle.device
        )[None, :].expand(middle.shape[1], -1)
        _paste(row, middle, specs[group * 3 + 2][1], 0, horizontal)
        rows.append(row)

    output = torch.zeros(
        (batch, height, width, channels),
        dtype=rows[0].dtype,
        device=rows[0].device,
    )
    _paste(output, rows[0], 0, specs[0][2])
    _paste(output, rows[1], 0, specs[3][2])
    vertical = _edge_feather(
        rows[2].shape[1], feather, leading=True, trailing=True, device=rows[2].device
    )[:, None].expand(-1, width)
    _paste(output, rows[2], 0, specs[6][2], vertical)
    return output


class SeamFixMcBoatyV2:
    """Focused compatibility port of MaraScott's tutorial-era McBoaty v2."""

    SIGMAS_TYPES = ["BasicScheduler", "SDTurboScheduler", "AlignYourStepsScheduler"]

    @classmethod
    def INPUT_TYPES(cls):
        upscale_models = [] if folder_paths is None else folder_paths.get_filename_list("upscale_models")
        try:
            import comfy.samplers

            samplers = comfy.samplers.KSampler.SAMPLERS
            schedulers = comfy.samplers.KSampler.SCHEDULERS
        except ImportError:
            samplers = []
            schedulers = []
        return {
            "hidden": {"id": "UNIQUE_ID"},
            "required": {
                "image": ("IMAGE",),
                "output_size": (
                    "BOOLEAN",
                    {"default": True, "label_on": "Upscale size", "label_off": "Input size"},
                ),
                "upscale_model": (upscale_models,),
                "feather_mask": (
                    "INT",
                    {
                        "default": 64,
                        "min": 0,
                        "max": 16384,
                        "step": 1,
                    },
                ),
                "model": ("MODEL",),
                "vae": ("VAE",),
                "vae_encode": (
                    "BOOLEAN",
                    {"default": True, "label_on": "tiled", "label_off": "standard"},
                ),
                "tile_size": (
                    "INT",
                    {"default": 512, "min": 320, "max": 4096, "step": 64},
                ),
                "seed": (
                    "INT",
                    {
                        "default": 4,
                        "min": 0,
                        "max": 0xFFFFFFFFFFFFFFFF,
                        "control_after_generate": True,
                    },
                ),
                "steps": ("INT", {"default": 10, "min": 1, "max": 10000}),
                "cfg": (
                    "FLOAT",
                    {"default": 2.5, "min": 0.0, "max": 100.0, "step": 0.1, "round": 0.01},
                ),
                "sigmas_type": (cls.SIGMAS_TYPES,),
                "sampler_name": (samplers,),
                "basic_scheduler": (schedulers,),
                "ays_model_type": (["SD1", "SDXL", "SVD"],),
                "positive": ("CONDITIONING",),
                "negative": ("CONDITIONING",),
                "denoise": (
                    "FLOAT",
                    {"default": 0.35, "min": 0.0, "max": 1.0, "step": 0.01},
                ),
            },
        }

    RETURN_TYPES = ("IMAGE", "IMAGE", "IMAGE", "STRING")
    RETURN_NAMES = ("image", "tiles", "original_resized", "info")
    FUNCTION = "fn"
    CATEGORY = CATEGORY
    DESCRIPTION = (
        "Self-contained tutorial-era McBoaty v2 port: 3x3 overlap tiling, model "
        "upscale, VAE encode, custom sampling, VAE decode, and feathered rebuild."
    )

    @staticmethod
    def _sigmas(
        sigmas_type: str,
        model: Any,
        steps: int,
        denoise: float,
        scheduler: str,
        model_type: str,
    ) -> torch.Tensor:
        from comfy_extras import nodes_custom_sampler

        if sigmas_type == "SDTurboScheduler":
            return nodes_custom_sampler.SDTurboScheduler.get_sigmas(
                model, steps, denoise
            )[0]
        if sigmas_type == "AlignYourStepsScheduler":
            from comfy_extras.nodes_align_your_steps import AlignYourStepsScheduler

            return AlignYourStepsScheduler().get_sigmas(model_type, steps, denoise)[0]
        return nodes_custom_sampler.BasicScheduler.get_sigmas(
            model, scheduler, steps, denoise
        )[0]

    def fn(
        self,
        image: torch.Tensor,
        output_size: bool,
        upscale_model: str,
        feather_mask: int,
        model: Any,
        vae: Any,
        vae_encode: bool,
        tile_size: int,
        seed: int,
        steps: int,
        cfg: float,
        sigmas_type: str,
        sampler_name: str,
        basic_scheduler: str,
        ays_model_type: str,
        positive: Any,
        negative: Any,
        denoise: float,
        **_hidden: Any,
    ):
        _require_comfy()
        if not isinstance(image, torch.Tensor) or image.ndim != 4:
            raise ValueError("McBoaty image must be a Comfy IMAGE tensor")
        from comfy_extras import nodes_custom_sampler, nodes_upscale_model

        input_height, input_width = int(image.shape[1]), int(image.shape[2])
        resized_width = math.ceil(input_width / 8) * 8
        resized_height = math.ceil(input_height / 8) * 8
        original = comfy_nodes.ImageScale().upscale(
            image, "nearest-exact", resized_width, resized_height, "center"
        )[0]

        loaded_upscaler = nodes_upscale_model.UpscaleModelLoader.load_model(upscale_model)[0]
        full_upscale = nodes_upscale_model.ImageUpscaleWithModel.upscale(
            loaded_upscaler, original
        )[0]
        output_height, output_width = int(full_upscale.shape[1]), int(full_upscale.shape[2])

        source_specs = _grid_specs(resized_width, resized_height)
        source_tiles = [
            original[:, y : y + height, x : x + width, :3]
            for _, x, y, width, height in source_specs
        ]
        upscaled_tiles = [
            nodes_upscale_model.ImageUpscaleWithModel.upscale(loaded_upscaler, tile)[0]
            for tile in source_tiles
        ]

        overlap = min(64, max(0, tile_size // 4))
        latent_tiles = []
        for tile in upscaled_tiles:
            if vae_encode:
                latent = comfy_nodes.VAEEncodeTiled().encode(
                    vae, tile, tile_size, overlap
                )[0]
            else:
                latent = comfy_nodes.VAEEncode().encode(vae, tile)[0]
            latent_tiles.append(latent)

        sampler = nodes_custom_sampler.KSamplerSelect.get_sampler(sampler_name)[0]
        sigmas = self._sigmas(
            sigmas_type,
            model,
            int(steps),
            float(denoise),
            basic_scheduler,
            ays_model_type,
        )
        sampled_tiles = [
            nodes_custom_sampler.SamplerCustom.sample(
                model,
                True,
                int(seed),
                float(cfg),
                positive,
                negative,
                sampler,
                sigmas,
                latent,
            )[0]
            for latent in latent_tiles
        ]

        decoded_tiles = []
        for latent in sampled_tiles:
            if vae_encode:
                decoded = comfy_nodes.VAEDecodeTiled().decode(
                    vae, latent, tile_size, overlap
                )[0]
            else:
                decoded = comfy_nodes.VAEDecode().decode(vae, latent)[0]
            decoded_tiles.append(decoded)

        output = _rebuild_nine_tiles(
            decoded_tiles, (output_height, output_width), int(feather_mask)
        ).clamp(0.0, 1.0)
        if not output_size:
            output = comfy_nodes.ImageScale().upscale(
                output,
                "nearest-exact",
                resized_width,
                resized_height,
                "center",
            )[0]
        tiles = torch.cat(decoded_tiles, dim=0)
        info = (
            f"Input: {resized_width}x{resized_height}; "
            f"output: {output.shape[2]}x{output.shape[1]}; "
            "nine overlapping McBoaty v2 tiles"
        )
        return output, tiles, original, info


TUTORIAL_NODE_CLASS_MAPPINGS = {
    # Original serialized names make the downloaded tutorial JSON load directly.
    "MarasitUpscalerRefinerNode_v2": SeamFixMcBoatyV2,
    "Image Resize": SeamFixImageResize,
    "PreviewBridge": SeamFixPreviewBridge,
    "ColorCorrect": SeamFixColorCorrect,
    # Namespaced aliases remain available when another suite owns a legacy name.
    "SeaminglyEpicMcBoatyV2": SeamFixMcBoatyV2,
    "SeaminglyEpicImageResize": SeamFixImageResize,
    "SeaminglyEpicMaskBridge": SeamFixPreviewBridge,
    "SeaminglyEpicColorCorrect": SeamFixColorCorrect,
}

TUTORIAL_NODE_DISPLAY_NAME_MAPPINGS = {
    "MarasitUpscalerRefinerNode_v2": "SeamFix — McBoaty v2 (bundled)",
    "Image Resize": "SeamFix — Image Resize (bundled)",
    "PreviewBridge": "SeamFix — Paint Mask Here",
    "ColorCorrect": "SeamFix — Color Correct (bundled)",
    "SeaminglyEpicMcBoatyV2": "Seamingly Epic — McBoaty v2",
    "SeaminglyEpicImageResize": "Seamingly Epic — Image Resize",
    "SeaminglyEpicMaskBridge": "Seamingly Epic — Paint Mask Here",
    "SeaminglyEpicColorCorrect": "Seamingly Epic — Color Correct",
}
