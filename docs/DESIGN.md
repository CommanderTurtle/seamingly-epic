# Design

## Guarantees and limits

The output PNG is encoded losslessly and keeps the source sample depth. RGB24
and RGBA32 use 8 bits per channel; RGB48 and RGBA64 use 16 bits per channel.
Alpha is copied exactly. The correction engine never convolves, resamples,
sharpens, or denoises the source image.

Field construction and gain application use `f64`. RGB48/RGBA64 never passes
through an 8-bit or float32 image representation, so the available source
sample lattice and its trillions of possible RGB combinations are retained.
The Comfy transport remains float32 end-to-end because that is the native
precision of the incoming Comfy `IMAGE` tensor.

Correction necessarily changes RGB values. "Zero loss" therefore means no
detail-destroying resampling or lossy encoding—not byte-identical pixels. A
photometric seam can be corrected without erasing texture; a semantic mismatch
between independently generated tiles cannot.

## Boundary model

For each candidate seam, the default scan walks every row of a vertical segment
or every column of a horizontal segment and samples narrow strips on both sides.
Pixels that are clipped, nearly black, transparent, highly textured, or
inconsistent with a persistent line are down-weighted. Each side is robustly
extrapolated to the boundary in log-linear RGB. A multiplicative
exposure/white-balance jump becomes an additive three-channel offset:

```text
d = log(right_at_boundary) - log(left_at_boundary)
```

For adjacent tiles `L` and `R`, correction gains satisfy:

```text
gain[R] - gain[L] = -d
```

All vertical and horizontal constraints are solved together, anchored so the
mean correction is zero. This avoids privileging one quadrant and prevents
endpoint gauges from accumulating around a 2x2 grid cycle. Graph gauges are
used only when accepted constraints connect every tile. A disconnected graph
falls back to symmetric local endpoint correction so it cannot create a new
join against an unmeasured neighbor. In neither case is a gauge applied as a
constant whole-tile correction.

This solve is global rather than a sequence of pairwise edits. A matrix-free,
Jacobi-preconditioned conjugate-gradient solve repeatedly applies the weighted
tile-graph Laplacian. Successive directions carry information through expanding
adjacency depths while retaining all earlier constraints; convergence couples
every tile in the connected component to every direct measurement and every
alternate path. A diagonally touching tile participates through its two
edge-connected paths rather than through an invented one-pixel corner sample.

The sparse representation stores only tiles and shared edges. Its memory is
`O(tiles + edges)`, rather than the `O(tiles^2)` matrix that would make large or
unusually shaped grids an artificial special case.

## Midpoint-anchored seam waves

The tile Laplacian values are endpoint gauges, not instructions to recolor an
entire tile. The stabilized boundary profile can differ at every position along
the seam. A one-dimensional low-pass operation removes noisy profile jitter;
that operation touches estimates, never source image samples.

For each profile sample, the graph-consistent part and its position-varying
remainder are split between the two sides so the difference of the resulting
endpoint corrections equals the negative measured jump. Each endpoint then
multiplies a raised-cosine normal wave:

```text
wave(0 at seam) = 1
wave(tile midpoint) = 0
wave(beyond midpoint) = 0
```

The wave and its first derivative are both continuous at the neutral midpoint.
Two boundaries of the same tile therefore approach the same zero anchor from
opposite sides without overlapping or requiring a user-selected feather width.
The outer half of every edge tile remains unaffected by that boundary's normal
wave as well.

Vertical and horizontal profiles can interact within one pixel of a grid
intersection. The engine repeatedly measures the correction difference at the
literal samples straddling each accepted join, projects the remaining residual
symmetrically into that boundary's two endpoints, and alternates axes. A pass is
accepted only if the maximum residual decreases. A worsening pass is restored
from its snapshot and retried with a smaller relaxation factor. This preserves
the best field found and removes fixed pass-count or strength tuning from the
normal interface.

The measurement is not a request to make two neighboring source pixels
identical, nor does it compare distant skin, sky, or shadow pixels as though
their scene illumination should match. Real gradients and edges can cross a
tile boundary. Four near/far strips are extrapolated to the join so the
inferred artificial discontinuity is removed while legitimate local gradients
remain. Distant tiles participate only in the sparse gauge solve, not through
invented pixel or semantic correspondences.

The final RGB operation is a per-pixel multiplication in linear light. There
is no source-space convolution and no mixing of one image sample into another.
There is no common negative log-gain: every normal wave reaches zero at its
tile midpoint, so the original exposure and white balance remain the reference.
An exactly clipped-white source channel is preserved exactly because it contains
no recoverable photometric magnitude from which a darker value could be inferred.

## Memory model

PNG is decoded scanline-by-scanline into a temporary memory-mapped raw file.
Analysis reads only seam bands. Each accepted seam position retains a target
difference and two RGB endpoints: nine `f64` values, or 72 bytes. Correction is
evaluated directly and applied in parallel in place; encoding walks the source
mmap sequentially. Field memory consequently scales with total seam length,
not image pixel count. Temporary source storage retains the exact original
sample representation.

The ComfyUI IMAGE node uses the same engine through a raw float32 descriptor.
The separate file node is the preferred path for 16K images because returning an
`IMAGE` tensor inherently asks ComfyUI to materialize the complete image.

## Tutorial-derived reference repair

The separate Reference Repair node intentionally does spatial work: it resizes
the original image, grows/feathers a mask, applies explicit manual color
controls, and composites that patch. This mirrors the cited workflow for
semantic artifacts. It is not mixed into automatic normalization, and its
detail-replacement behavior is visible and operator-controlled.
