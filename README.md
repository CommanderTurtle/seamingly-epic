# Seamingly Epic

A native rust program built for 8192x8192 (or higher) grid seamline removal for Nvidia Pixel DiT.

### Before:

![avif1](https://huggingface.co/sHEL1562/shelling/resolve/main/src/before1.avif)

### After:

![avif2](https://huggingface.co/sHEL1562/shelling/resolve/main/src/after1.avif)

For all starting out. Everything needed is in the /workflows/ folder.

Including [quick start](./workflows/quickstart.txt)

### Don't see it?

### Before:

![png1](https://huggingface.co/sHEL1562/shelling/resolve/main/src/before.png)

### After:

![png2](https://huggingface.co/sHEL1562/shelling/resolve/main/src/after.png)

---

Seamingly Epic corrects straight exposure and white-balance boundaries between
independently refined image tiles. It targets the exact case where four
1024-to-4096 PiD passes are assembled into one 8192x8192 image and the join at
`x=4096` or `y=4096` remains faintly visible.

## The whole method

Tiles are nodes and shared boundaries are sparse connections. The tile graph
reconciles what every neighbor observed, but its solution is used only to set
the two endpoint profiles at each real seam. Those profiles become smooth,
seam-normal waves which are full strength at the join and **exactly zero at the
center of each adjacent tile**. No correction is broadcast across a quadrant,
and there is no image-wide exposure gauge or Poisson cloud.

It is much like sparse attention for an image grid: the Laplacian solve carries
valid relationships through every adjacency depth, while detailed evidence
remains attached to the boundary that actually measured it. The result is a
deterministic graph-and-wave optimization rather than a learned model.

The ordinary zero-configuration invocation is therefore only:

```bash
seamingly-epic --x 4096 --y 4096 --in myfile.png --out fixed.png
```

When overlapping PiD renders are available, a second, independent structural
pass can replace the incompatible object geometry that a photometric field
cannot repair:

```powershell
.\target\release\seamingly-epic.exe strucfix `
  --x 4096 --y 4096 `
  --in "D:\output.png" --out "D:\outputfinal.png" `
  --xcross "D:\landscape.png" --ycross "D:\portrait.png"
```

For the standard 8192x8192 result, `landscape.png` is the 8192x4096 render
registered at `(0, 2048)`, and `portrait.png` is the 4096x8192 render registered
at `(2048, 0)`. `--xcross` means that the reference spans the X axis; it repairs
the horizontal `--y` join. `--ycross` spans the Y axis and repairs the vertical
`--x` join. The command validates those dimensions and placements exactly and
never guesses, resizes, or geometrically aligns a reference.

If only the exact cross intersection still needs alternate structural
evidence, a third independent pass accepts one seam-free center render:

```powershell
.\target\release\seamingly-epic.exe centerfix `
  --x 4096 --y 4096 `
  --in "D:\outputfinal.png" --out "D:\outputlast.png" `
  --center "D:\middle.png"
```

Here `--x/--y` are the reference center, just as they are the authoritative
intersection in the earlier commands. A 4096x4096 `middle.png` centered at
`4096,4096` therefore registers at `(2048,2048)` in an 8192x8192 base. The
reference is never resized or aligned heuristically.

Any number of comma-separated X and Y coordinates automatically defines the
tiles and every true shared boundary. No grid dimensions, adjacency depth,
sampling plan, or correction strength has to be supplied.

More formally, let the tile graph be $G=(V,E)$. Every tile $i\in V$ is a node.
Every measured shared boundary $(i,j)\in E$ is an edge with confidence weight
$w_{ij}$ and a robust log-linear RGB jump $\mathbf d_{ij}$. The unknown
three-channel log gain $\mathbf g_i$ for each tile should satisfy

$$
\mathbf g_j-\mathbf g_i \approx -\mathbf d_{ij}.
$$

All boundaries are reconciled at once by the zero-mean weighted least-squares
problem

$$
\mathbf g^\star =
\arg\min_{\sum_{i\in V}\mathbf g_i=\mathbf 0}
\sum_{(i,j)\in E} w_{ij}\,
\left\lVert (\mathbf g_j-\mathbf g_i)+\mathbf d_{ij} \right\rVert_2^2.
$$

With oriented incidence matrix $B$, diagonal edge-weight matrix $W$, and the
stacked boundary observations $\mathbf d$, this is the weighted graph
Laplacian system

$$
L_W\mathbf g=\mathbf b,
\qquad
L_W=B^{\mathsf T}WB,
\qquad
\mathbf b=-B^{\mathsf T}W\mathbf d.
$$

The native engine never materializes that matrix. It removes the constant-gain
nullspace with one temporary gauge anchor,

$$
\widetilde L_W=L_W+\alpha\mathbf e_0\mathbf e_0^{\mathsf T},
\qquad
\alpha=\max\!\left(1,\sum_{e\in E}w_e\right),
$$

solves each RGB channel, and recenters the result to zero mean. With Jacobi
preconditioner $M = \text{diag}(\widetilde L_W)$, its matrix-free
conjugate-gradient solve works in the Krylov spaces

$$
\mathcal K_k(M^{-1}\widetilde L_W,\,M^{-1}\mathbf b)=
\text{span}\{
M^{-1}\mathbf b,\;
(M^{-1}\widetilde L_W)\,M^{-1}\mathbf b,\;
(M^{-1}\widetilde L_W)^2\,M^{-1}\mathbf b,\;
\ldots,\;
(M^{-1}\widetilde L_W)^{k-1}\,M^{-1}\mathbf b
\}.
$$

One Laplacian multiplication communicates across one shared boundary;
successive directions retain earlier information while extending it through
another adjacency depth. At convergence, even opposite corners influence one
another through every real path and cycle in the connected tile graph. No
fictional diagonal or distant pixel comparison is introduced. Storage and
each graph iteration remain $O(|V|+|E|)$.

The graph values are not whole-tile gains. For an accepted edge $e=(a,b)$,
position $t$ along its seam, measured log-linear RGB jump $\mathbf d_e(t)$,
and solved tile gauges $\mathbf g_a,\mathbf g_b$, define

$$
\mathbf r_e(t)=\mathbf d_e(t)+(\mathbf g_b-\mathbf g_a).
$$

The correction values at the last sample of side $a$ and first sample of side
$b$ are

$$
\mathbf q_{e,a}(t)=\mathbf g_a+\tfrac12\mathbf r_e(t),
\qquad
\mathbf q_{e,b}(t)=\mathbf g_b-\tfrac12\mathbf r_e(t).
$$

Therefore their difference is exactly the negative measured seam step:

$$
\mathbf q_{e,b}(t)-\mathbf q_{e,a}(t)=-\mathbf d_e(t).
$$

Each endpoint is then carried inward with a raised-cosine (equivalently,
cosine-squared) wave. For normal distance $s$ from the seam and automatically
derived distance $h$ to that tile's midpoint,

```math \
\phi(s;h)=
\begin{cases}
\tfrac12[1+\cos(\pi s/h)]
  & \text{if } 0 \le s \lt h, \\[6pt]
0
  & \text{if } s \ge h.
\end{cases}
```

```math \
\alpha(d,t)=
\begin{cases}
\tfrac12\,[1+\cos\!\bigl(\pi d / D(t)\bigr)] & \text{if } 0 \le d < D(t)\

\[6pt]
0 & \text{if } d \ge D(t).
\end{cases}
```

Thus $\phi(0;h)=1$, $\phi(h;h)=0$, and the derivative is zero at both ends.
The seam closes at full strength without a hard transition, while every tile's
native midpoint and outer half remain untouched by that boundary's normal
wave. Vertical and horizontal waves sum where their supports meet. At grid
intersections, alternating projection
passes remeasure the literal $n=-1$ and $n=+1$ samples and adjust only the two
endpoint profiles. A speculative pass is retained only if it reduces the
maximum boundary residual; otherwise it is rolled back and retried with a
smaller relaxation factor.

For the set $\mathcal E(p)$ of boundary sides whose midpoint-bounded support
contains pixel $p$, the final correction is

$$
\mathbf c(p)=\sum_{(e,k)\in\mathcal E(p)}
\phi_e(p)\,\mathbf q_{e,k}(t_e(p)),
\qquad
\mathbf I_{\mathrm{out}}(p)=
\mathbf I_{\mathrm{in}}(p)\odot\exp(\mathbf c(p)).
$$

The field is represented by compact `f64` seam profiles and evaluated directly
for each output pixel. Source samples are never convolved with neighbors,
resampled, blurred, sharpened, denoised, regenerated, or passed through a
lower-precision image. There is deliberately no common negative headroom
shift: native tile interiors keep their exact source colors, and channels that
were exactly white remain exactly white.

Boundary analysis and output rows run in parallel with Rayon. `threads=0` uses
the platform pool (normally every available logical CPU). Decoded source pixels
live in a temporary memory map; compact endpoint profiles remain in memory.
Only PNG decode/encode is inherently sequential. No GPU runtime, model, tensor
conversion, or reduced precision is introduced.

The project provides four complementary repairs:

- **Native photometric correction** measures persistent boundary steps in
  linear light, solves globally consistent seam-endpoint gauges, and applies
  midpoint-anchored waves without blurring, resizing, denoising, or
  regenerating the source.
- **Registered Cross Structural Fix** consumes the landscape and portrait
  overlap renders from shifted PiD crops. Four unrestricted structural-match
  curves define an irregular cross. At each curve, canonical near/far bands
  match only the inner reference to the outer corrected base before a
  raised-cosine alpha wave is applied. Both references retain
  orthogonal-seam-aware weighting at their central overlap.
  This is the automatic path for fingers, edges, and other geometry that was
  generated differently on opposite sides of the original four-tile join.
- **Registered Center Structural Fix** is an optional third native pass for the
  one location where both cross references meet. Thousands of radial
  scanlines form a smooth star-shaped boundary around the center. The
  seam-free middle render is globally and boundary-matched to the authoritative
  input, remains opaque at the center, and fades to exact zero at that boundary.
  Nothing outside the star is changed.
- **Original SeamFix 2.1 workflow for ComfyUI** preserves Rob Adams' complete
  [45-node tutorial graph](https://www.youtube.com/watch?v=V-ASlpPI87Y),
  including both painted Softfix and HardFix lanes. Focused compatibility
  implementations of McBoaty v2, Image Resize, PreviewBridge, and ColorCorrect
  live in this custom-node project; KJNodes supplies only `GrowMaskWithBlur`.
- **Reference Repair for ComfyUI** remains as a compact PiD-ready helper when
  the already-refined image and original reference are the desired entrypoints.

The native path supports regular `1x2`, `2x1`, `2x2`, `5x5`, and larger grids,
plus arbitrary output-pixel X/Y seam coordinates. It is entirely local and has
no network, model, telemetry, or server dependency.

#### head over to [quick start](./workflows/quickstart.txt)

1. Pipe genned image latent endpoint directly into input (using this workflow json setup)
2. Gen full,horizontal,vertical,mid from the workflow - (~100s from prompt)
3. Run [quick start](./workflows/quickstart.txt) copypasta verbatim (i point to all 4 files in downloads folder) (~10s)
4. Enjoy 8k image from 2048 gen, thanks to Nvidia PiD

> Step 3 basically performs this order:

"Take image 1, perform color corrective pass on x,y. Take image 2(vert), do same with just y. Take image 3(horz), do same with just x. Apply the crossblobber (this is a structural pass). Take this output (image 4) and apply a feathered circle structural diff from the center of the centered image 5 (mid)." You're left with just image 6 in the end. Rust does this all in seconds. The powershell script in the included .txt just helps for programmatically doing this all automatically with 4 variable names; each is solely a `RightClick->Copy-File-Path` paste.

---

Build directly:

```bash
cargo build --release
```

The normal interface is deliberately just input, output, and exact seam lines:

```bash
seamingly-epic --x 4096 --y 4096 --in myfile.png --out fixed.png
```

Any number of comma-separated coordinates works. This example creates the six
regions of an irregular 3-column by 2-row grid and compares every shared edge
against its actual neighboring regions:

```bash
seamingly-epic --x 3084,5887 --y 4096 --in myfile.png --out fixed.png
```

Nothing else is required. For example, this derives a 5-column by 8-row layout
with 40 tiles and 67 separately scanwalked shared-edge segments:

```bash
seamingly-epic --x 222,3333,7755,8842 \
  --y 123,1234,2222,4444,5555,6666,7777 \
  --in myfile.png --out output.png
```

Coordinates in direct mode are exact (`refine_radius=0`). The `analyze` and
`correct` subcommands retain advanced controls and equal-grid shorthand.
Every shared edge is scanwalked at every position by default. Its varying
profile becomes two midpoint-anchored waves evaluated independently for every
output pixel. The matrix-free graph solve reconciles every grid neighbor and
propagates those relationships through every adjacency depth, but its values
exist only as seam endpoints—not constant edits to entire tiles. No waveform,
adjacency-depth, or tile-count setting is exposed to the user.

### Registered cross structural pass

Run the ordinary photometric correction first, unchanged:

```powershell
.\target\release\seamingly-epic.exe --x 4096 --y 4096 `
  --in "D:\file.png" --out "D:\output.png"
```

Then supply the two shifted overlap assemblies:

```powershell
.\target\release\seamingly-epic.exe strucfix `
  --x 4096 --y 4096 `
  --in "D:\output.png" --out "D:\outputfinal.png" `
  --xcross "D:\landscape.png" --ycross "D:\portrait.png"
```

The standard references come from these four 1024-source-pixel crops before
the same 1024-to-4096 PiD refinement:

```text
landscape / --xcross:
  (0,512,1024,1024) + (1024,512,1024,1024) -> 8192x4096

portrait / --ycross:
  (512,0,1024,1024) + (512,1024,1024,1024) -> 4096x8192
```

For every output row, the portrait path searches left and right for the lowest
base/reference structural difference. For every column, the landscape path
does the same above and below. Robust median and low-pass passes turn those
per-scanline positions into the four smooth, irregular boundaries shown in the
design diagram. The successful deep search is retained: for a standard
2048-pixel half-strip it can extend from 512 through 2015 pixels. There is no
256-pixel local-support cap; only the physical reference extent and safe
analysis margin limit the selected boundary.

Color is solved only after structure chooses that boundary. Four eight-pixel
log-linear bands are measured there: outer-far and outer-near come from the
already-corrected base, while inner-near and inner-far come from the alternate
reference. Both sides are extrapolated to the boundary using the same
near/far method as the canonical correction. Their difference is a one-sided
gain applied **only to the inner reference**. The base never receives this
correction. Huber stabilization and a continuous 96-pixel tangential low-pass
remove outlier stripes without filtering source pixels or splitting the gain
profile at the already-corrected quadrant join.

If `d` is normal distance from the original join and `D(t)` is the selected
structural-boundary distance at along-seam position `t`, the reference weight is

```math \
\alpha(d,t)=
\begin{cases}
\tfrac12\,[1+\cos!\bigl(\pi d / D(t)\bigr)] & \text{if } 0 \le d < D(t)\

\[6pt]
0 & \text{if } d \ge D(t).
\end{cases}
```

Thus the matched overlap render is opaque at the original structural seam and
approaches exact transparency at its irregular boundary. The corrected base is
completely authoritative at and beyond that boundary, and its RGB is never
modified by `strucfix`. Both value and first derivative meet smoothly. At the
central overlap, vertical and horizontal evidence use the smooth union
`1-(1-alpha_x)(1-alpha_y)`. Inside that union, each reference is down-weighted
as its own orthogonal PiD join is approached; the exact center is shared
symmetrically. No rectangular paste boundary is created. Outside the irregular
cross, the corrected base samples remain byte-for-byte unchanged. The four
`*_stitch` ranges in the JSON report describe those structural boundaries.

### Registered center structural pass

Do not replace either preceding pass. Run this only after `strucfix` when the
single cross intersection still needs the independently generated center crop:

```powershell
.\target\release\seamingly-epic.exe centerfix `
  --x 4096 --y 4096 `
  --in "D:\outputfinal.png" --out "D:\outputlast.png" `
  --center "D:\middle.png"
```

For the standard case, `middle.png` is the 4096x4096 PiD result made from the
centered `(512,512,1024,1024)` source crop. Its registration origin in the
8192x8192 image is `(2048,2048)`. Other even square center renders work the
same way: their dimensions and `--x/--y` determine placement without scaling.
`--mid` is accepted as an alias for `--center`.

The engine first obtains a robust global log-linear gain from sparse
same-coordinate comparisons. This changes only the reference and removes its
overall exposure/white-balance offset before structural matching. It then
searches outward on thousands of radial scanlines for the lowest
base/reference structural difference. Circular median and low-pass passes turn
those distances into one smooth, irregular star boundary without an angular
start/end seam.

At every point on the star, two outer near/far bands come from the canonical
`--in` image and two inner near/far bands come from `--center`. The canonical
extrapolation maps only the reference to the base. That angular boundary gain
is smoothed and joined to the robust center gain; it never becomes a gain on
the input image. For radius `r`, angle `theta`, and detected star radius
`R(theta)`, reference opacity is

```math \
\alpha(r,\theta)=
\begin{cases}
\tfrac12\,[1+\cos\!\bigl(\pi r / R(\theta)\bigr)] & \text{if } 0 \le r < R(\theta),\

\[6pt]
0 & \text{if } r \ge R(\theta).
\end{cases}
```

The seam-free reference is effectively opaque at dead center and becomes
exactly transparent at its data-derived edge. The already-perfect
`outputfinal.png` remains authoritative outside that edge. The pass performs
no resize, warp, spatial filter, or second correction of the base.

Analyze the standard 8192x8192 four-quadrant result without changing it:

```bash
seamingly-epic analyze assembled-8k.png --grid 2x2 --report seam-report.json
```

Apply the correction:

```bash
seamingly-epic correct assembled-8k.png assembled-8k-fixed.png \
  --grid 2x2 --report seam-report.json
```

Irregular boundaries are explicit output coordinates:

```bash
seamingly-epic correct input.png output.png \
  --no-grid --x-seams 3800,8250 --y-seams 4096
```

`COLSxROWS` is literal: `--grid 1x2` is one column and two rows (one horizontal
join); `--grid 2x1` is two columns and one row (one vertical join).

See [docs/CLI.md](docs/CLI.md) for settings, reports, 5x5 usage, storage
requirements, and supported PNG formats.

## What is and is not preserved

- PNG output remains losslessly encoded at the source per-channel depth:
  RGB24/RGBA32 use 8-bit channels; RGB48/RGBA64 use 16-bit channels.
- The encoder reads the source IDAT zlib `FLEVEL` class and reuses its
  representative DEFLATE strength instead of forcing every output through a
  Balanced preset. If analysis accepts no correction, the input PNG is copied
  byte-for-byte rather than decoded and re-encoded.
- All analysis and correction-field arithmetic is `f64`; a 16-bit source is
  never routed through an 8-bit or float32 image, palette, or intermediate
  codec. The Comfy path returns float32 only because its source `IMAGE` is
  already float32.
- Explicit alpha samples are copied byte-for-byte; RGB hidden behind effectively
  transparent alpha is also left untouched.
- Standard color, text, EXIF, ICC, and ComfyUI workflow/prompt metadata exposed
  by the PNG codec are retained.
- The ordinary photometric path never filters, mixes, or resamples spatial
  detail. `strucfix` intentionally mixes only same-coordinate samples from the
  registered overlap renders; it still performs no resize, warp, convolution,
  denoise, or lossy encode. `centerfix` follows the same preservation contract
  with one registered center render.
- Every inferred normal wave is exactly zero at its tile midpoint, there is no
  image-wide exposure shift, and exact source-white channels remain white.

Correction necessarily changes RGB values. Here, "lossless" means no lossy
codec or detail-destroying spatial operation—not byte-identical color samples.
PNG does not store an encoder's exact implementation or literal 0-9 setting,
so a corrected file cannot be promised the same byte count as its source. A
smaller corrected PNG is still lossless; the meaningful invariants are sample
depth, color type, alpha, recognized metadata, dimensions, and pixel values.
An exposure/white-balance discontinuity is suitable for the ordinary native
engine. An object split or double edge can use `strucfix` when the two shifted
overlap renders exist; the optional `centerfix` pass consumes the final
seam-free middle render when the cross intersection alone remains unresolved.
Other semantic damage belongs in the painted Reference Repair workflow or
another generative edit.

The method and its limits are documented in [docs/DESIGN.md](docs/DESIGN.md).
The video reconstruction, timestamps, and scientific references are recorded
in [docs/RESEARCH.md](docs/RESEARCH.md).

## Verification

The repository's non-Comfy checks are:

```bash
cargo fmt --all --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo audit
python -m compileall -q __init__.py nodes.py tutorial_nodes.py runtime.py
cargo build --release --locked
```

These checks validate the native implementation's complete build graph, Rust
advisory graph, and Python syntax without inventing a synthetic photographic
quality test. There is no project-owned Python dependency graph to audit:
ComfyUI supplies Python, PyTorch, and NumPy. A real ComfyUI launch remains the
final environment-specific confirmation because the custom nodes intentionally
depend on ComfyUI's own runtime.

`cargo audit` is an optional maintainer check provided by `cargo-audit`; it is
not installed or required by either setup script or at runtime.
