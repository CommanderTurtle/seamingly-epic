# Design

## Guarantees and limits

The output PNG is encoded losslessly and keeps the source sample depth. RGB24
and RGBA32 use 8 bits per channel; RGB48 and RGBA64 use 16 bits per channel.
Alpha is copied exactly. The correction engine never convolves, resamples,
sharpens, or denoises the source image.

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
corrections from accumulating around a 2x2 grid cycle. Global tile gains are
used only when accepted constraints connect every tile. A disconnected graph
falls back to bounded local correction so it cannot create a new join against
an unmeasured neighbor.

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

## Residual field

After global tile gains, the remaining boundary profile can differ at every
position along the seam. A robust one-dimensional profile is smoothed along the
boundary, split symmetrically between both sides, and faded inward with a
raised-cosine ramp. At an intersection, the X- and Y-derived fields add. The
engine evaluates this two-dimensional "smokemap" independently for every output
pixel; only the correction field is smoothed, never the image or its spatial
detail.

The global field and local field have deliberately different evidence. Global
per-tile gains contain the all-depth graph relationship. The position-varying
residual is supported by the specific shared scanline that measured it and is
feathered into its two incident tiles. Broadcasting one local texture-dependent
residual into a distant tile would invent a color observation that does not
exist.

The comparison is not a request to make two neighboring source pixels
identical. Real scene gradients and edges can cross a tile boundary. Four
near/far strips are extrapolated to the join so the inferred discontinuity is
removed while the legitimate local gradient remains.

## Memory model

PNG is decoded scanline-by-scanline into a temporary memory-mapped raw file.
Analysis reads only seam bands. Correction is applied in parallel in place.
Encoding then walks the mmap sequentially. Peak resident memory is bounded by
working strips and OS-managed mmap pages rather than the decoded image size.

The ComfyUI IMAGE node uses the same engine through a raw float32 descriptor.
The separate file node is the preferred path for 16K images because returning an
`IMAGE` tensor inherently asks ComfyUI to materialize the complete image.

## Tutorial-derived reference repair

The separate Reference Repair node intentionally does spatial work: it resizes
the original image, grows/feathers a mask, applies explicit manual color
controls, and composites that patch. This mirrors the cited workflow for
semantic artifacts. It is not mixed into automatic normalization, and its
detail-replacement behavior is visible and operator-controlled.
