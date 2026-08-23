# Seamingly Epic

Seamingly Epic corrects straight exposure and white-balance boundaries between
independently refined image tiles. It targets the exact case where four
1024-to-4096 PiD passes are assembled into one 8192x8192 image and the join at
`x=4096` or `y=4096` remains faintly visible.

## The whole method

Tiles are nodes and shared boundaries are sparse connections. That small graph
establishes a globally consistent exposure/white-balance gauge. Pixels then
become the nodes of a second graph: every measured seam sample is injected as
a desired edge gradient, and one full-resolution `f64` inverse-Laplacian solve
turns all of those measurements into a single image-wide correction cloud. It
is much like sparse attention for an image grid, except the result is an exact,
deterministic graph optimization rather than a learned model.

The ordinary zero-configuration invocation is therefore only:

```bash
seamingly-epic --x 4096 --y 4096 --in myfile.png --out fixed.png
```

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
\mathbf g^\star=
\underset{\sum_{i\in V}\mathbf g_i=\mathbf 0}{\operatorname{arg\,min}}
\sum_{(i,j)\in E} w_{ij}
\left\|\left(\mathbf g_j-\mathbf g_i\right)+\mathbf d_{ij}\right\|_2^2.
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
preconditioner $M=\operatorname{diag}(\widetilde L_W)$, its matrix-free
conjugate-gradient solve works in the Krylov spaces

$$
\mathcal K_k(M^{-1}\widetilde L_W,M^{-1}\mathbf b)=
\operatorname{span}\!\left\{
M^{-1}\mathbf b,
(M^{-1}\widetilde L_W)M^{-1}\mathbf b,
(M^{-1}\widetilde L_W)^2M^{-1}\mathbf b,
\ldots,
(M^{-1}\widetilde L_W)^{k-1}M^{-1}\mathbf b
\right\}.
$$

One Laplacian multiplication communicates across one shared boundary;
successive directions retain earlier information while extending it through
another adjacency depth. At convergence, even opposite corners influence one
another through every real path and cycle in the connected tile graph. No
fictional diagonal or distant pixel comparison is introduced. Storage and
each graph iteration remain $O(|V|+|E|)$.

The tile solve handles the globally consistent constant part. It leaves a
position-varying residual profile $\mathbf r_e(t)$ at each accepted seam. Now
let $G_\Omega=(\Omega,E_\Omega)$ be the four-neighbor graph of **every output
pixel**, and let $D$ be its oriented incidence matrix. A desired correction
gradient $\mathbf v_{pq}$ is zero on ordinary pixel edges and equals the
negative measured residual on the edge that crosses a seam. The dense field is
the zero-mean gradient-domain solution

$$
\mathbf h^\star=
\underset{\sum_{p\in\Omega}\mathbf h(p)=\mathbf 0}
{\operatorname{arg\,min}}
\sum_{(p,q)\in E_\Omega}
\left\|\left(\mathbf h(q)-\mathbf h(p)\right)-
\mathbf v_{pq}\right\|_2^2,
$$

or equivalently

$$
L_\Omega\mathbf h=D^{\mathsf T}\mathbf v,
\qquad
L_\Omega=D^{\mathsf T}D,
\qquad
\mathbf h=L_\Omega^{+}D^{\mathsf T}\mathbf v.
$$

This is the literal ghost-map construction. By linearity, if $a$ enumerates
the accepted per-position seam impulses,

$$
\mathbf h=
\sum_a L_\Omega^{+}D^{\mathsf T}\mathbf e_a\,\mathbf v_a.
$$

Each summand is the conceptual full-image ghost field caused by one seam
observation. The implementation does not allocate thousands of duplicate
images; it superposes them exactly in spectral space and stores their single
sum. For an 8x12 layout, the sparse measurements therefore induce all
$96^2=9216$ ordered tile relationships while the actual solve remains over the
real pixel graph. Tile 1 and tile 96 influence one another through every
intervening path without falsely asserting that two unrelated scene pixels
must have the same color.

With zero-flux outer boundaries, the rectangular pixel Laplacian is exactly
diagonalized by a two-dimensional DCT-II. Its eigenvalues are

$$
\lambda_{k\ell}=
4\sin^2\!\left(\frac{\pi k}{2W}\right)+
4\sin^2\!\left(\frac{\pi \ell}{2H}\right).
$$

The DC coefficient is set to zero to fix the otherwise arbitrary common
exposure. DCT-III reconstructs one independent `f64` value per pixel and RGB
channel. A small raised-cosine closure field then supplies only the
non-integrable remainder at the exact two samples straddling each seam. For
normal distance $n$ and width $w$, its support is

$$
\phi(n)=
\begin{cases}
\tfrac12[1+\cos(\pi|n|/w)],&|n|<w,\\
0,&|n|\ge w.
\end{cases}
$$

For tile $\tau(p)$, global field $\mathbf h$, closure field $\boldsymbol\ell$,
and a common highlight-preserving gauge $a$, the final correction is

$$
\mathbf c(p)=\mathbf g_{\tau(p)}+\mathbf h(p)+
\boldsymbol\ell(p)+a\mathbf 1,
\qquad
\mathbf I_{\mathrm{out}}(p)=
\mathbf I_{\mathrm{in}}(p)\odot\exp(\mathbf c(p)).
$$

That correction field is the two-dimensional **smokemap**. Only its values are
reconstructed. Source samples are never convolved with neighbors, resampled,
blurred, sharpened, denoised, regenerated, or passed through a lower-precision
image. If the predicted correction would exceed the encoding ceiling, the
single common gauge $a\le0$ shifts all channels equally; this preserves the
solved color relationships and avoids destructive per-channel highlight
clipping.

Boundary analysis, DCT rows and columns, transposes, correction-field storage,
headroom inspection, and output rows run in parallel with Rayon. `threads=0`
uses the platform pool (normally every available logical CPU). RustDCT uses
RustFFT's native SIMD-capable kernels, while decoded source pixels and the final
three-plane field live in temporary memory maps. Only PNG decode/encode remains
inherently sequential. No GPU runtime, model, tensor conversion, or reduced
precision is introduced.

The project provides two complementary repairs:

- **Native photometric correction** measures persistent boundary steps in
  linear light, solves globally consistent tile gains, and applies a smooth
  residual field without blurring, resizing, denoising, or regenerating the
  source.
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

## Install as a ComfyUI custom node

Clone this repository directly under `ComfyUI/custom_nodes`, then build its
small Rust binary once.

Windows PowerShell:

```powershell
Set-Location C:\path\to\ComfyUI\custom_nodes\ComfyUI-Seamingly-Epic
.\setup.ps1
```

Linux:

```bash
cd /path/to/ComfyUI/custom_nodes/ComfyUI-Seamingly-Epic
./setup.sh
```

Restart ComfyUI. The nodes appear under `image / seamingly epic`:

- **Seamingly Epic — Native IMAGE**: float32 in/out, correction-area mask, and
  JSON diagnostics. Use it inside an ordinary workflow.
- **Seamingly Epic — Streaming PNG**: memory-mapped
  RGB24/RGBA32/RGB48/RGBA64 PNG path. Use it for 8K/16K images that should not
  become another full Comfy tensor.
- **Seamingly Epic — Reference Repair**: the tutorial's painted-reference
  composite path in one node.

The complete tutorial workflow is tracked at
[`workflows/SeamFixVer2.1.original.json`](workflows/SeamFixVer2.1.original.json).
It is the original 45-node/41-link JSON payload, not a visually similar
reconstruction. Import it directly. The legacy serialized node type names are
registered by this pack, so MaraScott, WAS Node Suite, Impact Pack, and Art
Venture are not required. Install only current ComfyUI plus KJNodes; VAE Utils
may remain installed for the surrounding PiD workflow.

ComfyUI already supplies Python, PyTorch, and NumPy; the project adds no Python
packages. Rust/Cargo is only needed to run the setup script. See
[docs/COMFYUI.md](docs/COMFYUI.md) for exact wiring and mask behavior.

## Native CLI

Build directly:

```bash
cargo build --release --locked
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
residual becomes a smooth correction field evaluated independently for every
output pixel, while the globally solved per-tile gains reconcile all grid
neighbors and intersections together. The matrix-free graph solve propagates
those relationships through every adjacency depth until the complete connected
grid agrees; no multiscale or tile-count setting is exposed to the user.

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
- Source geometry and spatial detail are never filtered, mixed, or resampled.

Correction necessarily changes RGB values. Here, "lossless" means no lossy
codec or detail-destroying spatial operation—not byte-identical color samples.
PNG does not store an encoder's exact implementation or literal 0-9 setting,
so a corrected file cannot be promised the same byte count as its source. A
smaller corrected PNG is still lossless; the meaningful invariants are sample
depth, color type, alpha, recognized metadata, dimensions, and pixel values.
An exposure/white-balance discontinuity is suitable for the native engine; an
object split, double edge, or incompatible generated texture belongs in the
Reference Repair node or another generative edit.

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
