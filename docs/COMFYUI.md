# ComfyUI workflow guide

## Native IMAGE

Use **Seamingly Epic — Native IMAGE** immediately after the four PiD outputs
have been concatenated. For the standard 8192x8192 result:

1. Connect the assembled `IMAGE`.
2. Leave `grid_columns=2` and `grid_rows=2`.
3. Leave X/Y strings empty; the node derives `4096` on both axes.
4. Leave `refine_radius=0` when the concatenate coordinate is exact. Increase
   it only when a prior crop/resize may have shifted the line.
5. Inspect `correction_area` and `report_json`. A rejected segment is left
   unchanged.

The default `sample_stride=1` scanwalks every Y position on vertical segments
and every X position on horizontal segments. A full-resolution `f64` Neumann
Poisson solve superposes every accepted sample into one global correction
field, and the native engine evaluates that field for every output pixel.

The IMAGE path writes the tensor as little-endian float32, invokes the Rust
engine without a shell, and reads float32 back. It never quantizes through an
8-bit interchange image. ComfyUI still holds the input and output tensors, so
this path is convenient rather than memory-minimal.

The correction-area MASK is an **evidence/closure diagnostic**, not the support
of the global field (which is image-wide). Brightness is accepted boundary
confidence multiplied by the raised-cosine support of the exact residual
closure term. Confidence gates unreliable segments; it does not attenuate an
accepted residual and knowingly leave part of it behind.

## Streaming PNG

Use **Seamingly Epic — Streaming PNG** for a final 8K/16K PNG already saved to
ComfyUI's input directory.

1. Upload/select the PNG.
2. Configure the same grid or explicit seam coordinates.
3. Choose an output prefix.
4. The node writes a unique PNG to ComfyUI's output directory and returns its
   absolute path plus the JSON report.

The node never creates an `IMAGE` tensor. Its preview refers to the completed
output file. Source decoding and the completed `f64` field use temporary memory
maps; the spectral solve uses two full-resolution `f64` work planes for one
color channel at a time. See the exact storage figures in `docs/CLI.md`.

## Complete SeamFix 2.1 tutorial workflow

Rob Adams' [Fixing the Seams from a Tiled Upscaler in
ComfyUI](https://www.youtube.com/watch?v=V-ASlpPI87Y) demonstrates a different
class of repair. The original image is resized, optionally blurred and
color-adjusted, then composited through a grown/blurred hand-painted mask. This
can replace a double edge or hallucinated object but may replace generated
detail too.

Import [`../workflows/SeamFixVer2.1.original.json`](../workflows/SeamFixVer2.1.original.json).
The payload preserves the original workflow exactly:

- 45 nodes and 41 links;
- the first McBoaty pass at `denoise=0.35`;
- the painted **Softfix** lane with mask blur radius `20`, brightness `-3`,
  and saturation `-6`;
- the second McBoaty pass at `denoise=0.5`;
- the painted **HardFix** lane with mask blur radius `10`, temperature `2`,
  brightness `-3`, and saturation `-5`.

The original graph refers to four historical custom-node names. This project
registers focused compatibility implementations under those exact serialized
names:

| Workflow type | Bundled implementation |
| --- | --- |
| `MarasitUpscalerRefinerNode_v2` | nine-overlap-tile McBoaty v2 upscale/refine/rebuild |
| `Image Resize` | WAS-compatible resize and optional 8x supersampling |
| `PreviewBridge` | current ComfyUI Mask Editor bridge |
| `ColorCorrect` | all six original color controls without OpenCV |

Core ComfyUI supplies the checkpoint, CLIP, image, mask-conversion, blur,
composite, reroute, preview, and note nodes. KJNodes supplies
`GrowMaskWithBlur`. VAE Utils is compatible with the surrounding PiD graph but
the untouched tutorial JSON does not require one of its nodes.

### Paint each mask

For each node displayed as **SeamFix — Paint Mask Here**:

1. Queue once so the node has an image preview.
2. Right-click that preview and choose **Open in Mask Editor**.
3. Paint the seam or semantic artifact exactly as in the video.
4. Save to the node.
5. Queue again. The node reads the clipspace PNG's inverted alpha as its MASK.

This is an actual interactive mask round-trip. It is not an automatically
generated seam strip standing in for the tutorial's painter.

The bundled McBoaty node preserves the tutorial controls, but the JSON's
checkpoint, input-image, and upscaler filenames are the author's local choices.
Select files available in the current ComfyUI installation before queueing.

## Compact Reference Repair

**Seamingly Epic — Reference Repair** is a separate convenience entrypoint for
an already-refined PiD result. It condenses only the reference-resize,
mask-grow/feather, color-adjust, and masked-composite portion:

```text
refined IMAGE ────────────────────────────────┐
                                             ├─ masked composite → repaired IMAGE
original/reference IMAGE → resize → color ───┘
painted MASK or generated grid → grow → feather
```

Mask modes:

- `provided_or_grid`: use a connected mask; otherwise generate strips around
  the configured grid. This is the practical default.
- `provided_plus_grid`: union the painted mask with generated seam strips.
- `grid_only`: ignore a connected mask.
- `provided_only`: require a connected mask.

`grow_pixels` expands the region with separable max pooling.
`feather_pixels` applies a three-pass separable box approximation to a broad
Gaussian fade. `reference_blur`, exposure, saturation, and temperature mirror
the tutorial's manual source-patch preparation. They default to neutral except
for the mask grow/feather.

For only a white-balance line, use Native IMAGE or Streaming PNG first. Use
Reference Repair when the pixels on the two sides no longer depict compatible
content.

## Binary and scratch overrides

Setup copies the release binary into this repository's ignored `bin` folder.
Two optional environment variables are available:

```text
SEAMINGLY_EPIC_BIN   absolute path to a different native binary
SEAMINGLY_EPIC_TEMP  scratch root for source staging, f64 fields, and IMAGE transport
```

The nodes do not launch a server, bind a port, or contact the internet. The
paint bridge retains only the clipspace filename saved in the workflow node;
ComfyUI owns the corresponding input image just as it does for its native Mask
Editor.
