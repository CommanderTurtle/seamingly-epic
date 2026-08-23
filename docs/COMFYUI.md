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
and every X position on horizontal segments. The native engine then evaluates
the combined global and position-varying correction field for every output
pixel.

The IMAGE path writes the tensor as little-endian float32, invokes the Rust
engine without a shell, and reads float32 back. It never quantizes through an
8-bit interchange image. ComfyUI still holds the input and output tensors, so
this path is convenient rather than memory-minimal.

The correction-area MASK is diagnostic: brightness is the accepted boundary
confidence multiplied by the same raised-cosine spatial support used by the
engine. Confidence gates unreliable segments; it does not attenuate an accepted
residual and knowingly leave part of it behind.

## Streaming PNG

Use **Seamingly Epic — Streaming PNG** for a final 8K/16K PNG already saved to
ComfyUI's input directory.

1. Upload/select the PNG.
2. Configure the same grid or explicit seam coordinates.
3. Choose an output prefix.
4. The node writes a unique PNG to ComfyUI's output directory and returns its
   absolute path plus the JSON report.

The node never creates an `IMAGE` tensor. Its preview refers to the completed
output file, while decoding, correction, and encoding stay bounded in the Rust
process.

## Reference Repair

Rob Adams' [Fixing the Seams from a Tiled Upscaler in
ComfyUI](https://www.youtube.com/watch?v=V-ASlpPI87Y) demonstrates a different
class of repair. The original image is resized, optionally blurred and
color-adjusted, then composited through a grown/blurred hand-painted mask. This
can replace a double edge or hallucinated object but may replace generated
detail too.

**Seamingly Epic — Reference Repair** reproduces that relevant 45-node workflow
chain in one native PyTorch node:

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
SEAMINGLY_EPIC_TEMP  scratch directory for IMAGE float32 transport
```

The nodes do not launch a server, bind a port, contact the internet, or retain
state after execution.
