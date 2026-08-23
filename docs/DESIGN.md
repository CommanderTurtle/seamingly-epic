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

## Full-resolution ghost field

After global tile gains, the remaining boundary profile can differ at every
position along the seam. The profile itself is stabilized with robust weights
and a one-dimensional low-pass operation. That operation touches estimates,
not image samples.

Every output pixel becomes a node in a four-neighbor graph. The desired
correction gradient is zero on ordinary edges and is the negative residual on
each accepted edge that crosses a seam. The engine solves

```text
minimize sum_edges || (h[q] - h[p]) - desired_gradient[p,q] ||^2
subject to mean(h) = 0
```

independently for the three log-linear RGB channels. Equivalently,
`L h = D^T v`, where `D` is the pixel-incidence matrix and `L=D^T D` is the
Neumann pixel Laplacian. The rectangular-grid eigenvalues are known exactly, so
a two-dimensional DCT-II, elementwise inverse eigenvalue, and DCT-III solve the
system without an iterative stopping approximation. The sole nullspace mode is
the common exposure; its DCT coefficient is fixed to zero.

The inverse Laplacian is dense. Consequently every accepted seam position
influences every output pixel, including diagonally and remotely related tiles.
This is mathematically identical to constructing one full-image Green's-function
"ghost map" per seam impulse and adding all of them:

```text
h = sum_impulses pseudoinverse(L) * impulse
```

Only the final sum is stored. Explicitly materializing the separate maps—or a
tile-count-squared family of intermediate images—would consume far more memory
and produce the same field by linear superposition.

The projection cannot reproduce a non-integrable set of varying X/Y gradients
at every crossing exactly. After reconstruction, the engine measures the
remaining error at the literal samples on both sides of each seam. A symmetric
raised-cosine field closes only that remainder. Thus the global cloud supplies
all-depth influence, and the local term enforces the measured boundary without
discarding the globally reconstructed component.

The comparison is not a request to make two neighboring source pixels
identical, nor does it compare distant skin, sky, or shadow pixels as though
their scene illumination should match. Real gradients and edges can cross a
tile boundary. Four near/far strips are extrapolated to the join so the
inferred artificial discontinuity is removed while legitimate local gradients
remain. Distant regions participate through the solved photometric field, not
through invented semantic correspondences.

The final RGB operation is a per-pixel multiplication in linear light. There
is no source-space convolution and no mixing of one image sample into another.
A common negative log-gain is added only when necessary to keep every predicted
channel within the output encoding ceiling. Because this gauge is identical in
R, G, and B, it cannot alter the solved white balance or seam differences.

## Memory model

PNG is decoded scanline-by-scanline into a temporary memory-mapped raw file.
Analysis reads only seam bands. The completed RGB field is held as three
temporary memory-mapped `f64` planes: exactly 24 bytes per output pixel. One
channel is solved at a time with two in-memory `f64` work planes for the DCT and
transpose. Correction is then applied in parallel in place, and encoding walks
the source mmap sequentially.

For 8192x8192, each `f64` plane is 512 MiB, the stored RGB field is 1.5 GiB, and
the spectral work pair is 1 GiB. For 16384x16384 those figures are 2 GiB,
6 GiB, and 4 GiB. Temporary source storage is additional and retains the exact
original sample representation. This is intentionally a compute-and-storage
heavy fidelity path, not a reduced-resolution approximation.

The ComfyUI IMAGE node uses the same engine through a raw float32 descriptor.
The separate file node is the preferred path for 16K images because returning an
`IMAGE` tensor inherently asks ComfyUI to materialize the complete image.

## Tutorial-derived reference repair

The separate Reference Repair node intentionally does spatial work: it resizes
the original image, grows/feathers a mask, applies explicit manual color
controls, and composites that patch. This mirrors the cited workflow for
semantic artifacts. It is not mixed into automatic normalization, and its
detail-replacement behavior is visible and operator-controlled.
