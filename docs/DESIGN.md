# Design

## Guarantees and limits

The output PNG is encoded losslessly and keeps the source bit depth. Alpha is
copied exactly. The correction engine never convolves, resamples, sharpens, or
denoises the source image.

Correction necessarily changes RGB values. "Zero loss" therefore means no
detail-destroying resampling or lossy encoding—not byte-identical pixels. A
photometric seam can be corrected without erasing texture; a semantic mismatch
between independently generated tiles cannot.

## Boundary model

For each candidate seam, samples are taken in narrow strips on both sides.
Pixels that are clipped, nearly black, highly textured, or inconsistent with a
persistent line are down-weighted. Each side is robustly extrapolated to the
boundary in log-linear RGB. A multiplicative exposure/white-balance jump becomes
an additive three-channel offset:

```text
d = log(right_at_boundary) - log(left_at_boundary)
```

For adjacent tiles `L` and `R`, correction gains satisfy:

```text
gain[R] - gain[L] = -d
```

All vertical and horizontal constraints are solved together, anchored so the
weighted mean correction is zero. This avoids privileging one quadrant and
prevents corrections from accumulating around a 2x2 grid cycle.

## Residual field

After global tile gains, a small remaining boundary profile may vary along the
seam. A robust one-dimensional profile is smoothed along the boundary, split
symmetrically between both sides, and faded to zero with a raised-cosine ramp.
The field changes color/exposure only; source spatial frequencies are never
filtered.

## Memory model

PNG is decoded scanline-by-scanline into a temporary memory-mapped raw file.
Analysis reads only seam bands. Correction is applied in parallel in place.
Encoding then walks the mmap sequentially. Peak resident memory is bounded by
working strips and OS-managed mmap pages rather than the decoded image size.

The ComfyUI IMAGE node uses the same engine through a raw float32 descriptor.
The separate file node is the preferred path for 16K images because returning an
`IMAGE` tensor inherently asks ComfyUI to materialize the complete image.

