# Seamingly Epic

Seamingly Epic corrects straight exposure and white-balance boundaries between
independently refined image tiles. It targets the exact case where four
1024-to-4096 PiD passes are assembled into one 8192x8192 image and the join at
`x=4096` or `y=4096` remains faintly visible.

## The whole method

Tiles are nodes, shared boundaries are sparse connections, and successive
Laplacian iterations expand the effective receptive field until distant tiles
participate in the global solution—while retaining higher-resolution local
seam profiles. It is much like sparse attention for an image grid, except the
result is a deterministic graph optimization rather than a learned model.

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

The graph solve handles the globally consistent part of the correction. The
remaining position-varying mismatch stays attached to the boundary that
actually measured it. For output pixel $p$, tile $\tau(p)$, nearby measured
edges $\mathcal N(\tau(p))$, along-edge coordinate $t_e(p)$, signed half-split
$s_e(p)\in\{-\tfrac12,+\tfrac12\}$, residual profile $\mathbf r_e$, and
raised-cosine support $\phi_e$, the complete per-pixel log correction is

$$
\mathbf c(p)=\mathbf g_{\tau(p)}+
\sum_{e\in\mathcal N(\tau(p))}
s_e(p)\,\phi_e(p)\,\mathbf r_e\!\left(t_e(p)\right),
$$

where, for normal distance $n_e(p)$ and feather half-width $h_e$,

$$
\phi_e(p)=
\begin{cases}
\tfrac12\!\left[1+\cos\!\left(\pi |n_e(p)|/h_e\right)\right],
& |n_e(p)|<h_e,\\[4pt]
0, & |n_e(p)|\ge h_e.
\end{cases}
$$

The source is then changed only photometrically in linear light:

$$
\mathbf I_{\mathrm{out}}(p)=
\exp\!\left(\log\mathbf I_{\mathrm{in}}(p)+\mathbf c(p)\right).
$$

This per-pixel field is the two-dimensional **smokemap**: a continuous map
derived from every accepted scanline and every globally related tile. The
correction field is smoothed; the image is never blurred, resampled, denoised,
or regenerated. Local seam evidence remains local because broadcasting it to
a distant tile would invent a measurement, while the Laplacian gain solve
already carries the valid all-depth relationship.

Boundary segments and output rows run in parallel with Rayon. `threads=0`
uses the platform pool (normally every available logical CPU), decoded PNG data
is held in a bounded memory-mapped backing file, and only inherently sequential
PNG decode/encode stages stay serial. The sparse graph solve is tiny beside the
pixel pass. A GPU is intentionally unnecessary: transferring this deterministic
element-wise correction through CUDA would not improve its mathematics, color
fidelity, or output quality.

The project provides two complementary repairs:

- **Native photometric correction** measures persistent boundary steps in
  linear light, solves globally consistent tile gains, and applies a smooth
  residual field without blurring, resizing, denoising, or regenerating the
  source.
- **Reference Repair for ComfyUI** condenses Rob Adams' manual
  [SeamFix 2.1 workflow](https://www.youtube.com/watch?v=V-ASlpPI87Y) into one
  node: resize the original reference, grow/feather a supplied or generated
  mask, color-adjust, and composite it over a semantic artifact.

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
- **Seamingly Epic — Streaming PNG**: bounded-memory
  RGB24/RGBA32/RGB48/RGBA64 PNG path. Use it for 8K/16K images that should not
  become another full Comfy tensor.
- **Seamingly Epic — Reference Repair**: the tutorial's painted-reference
  composite path in one node.

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
- Explicit alpha samples are copied byte-for-byte.
- Standard color, text, EXIF, ICC, and ComfyUI workflow/prompt metadata exposed
  by the PNG codec are retained.
- Source geometry and spatial detail are never filtered or resampled.

Correction necessarily changes RGB values. Here, "lossless" means no lossy
codec or detail-destroying spatial operation—not byte-identical color samples.
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
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo audit
python -m compileall -q __init__.py nodes.py runtime.py
cargo build --release --locked
```

These checks validate the native implementation, Rust advisory graph, and
Python syntax. There is no project-owned Python dependency graph to audit:
ComfyUI supplies Python, PyTorch, and NumPy. A real ComfyUI launch remains the
final environment-specific confirmation because the custom nodes intentionally
depend on ComfyUI's own runtime.

`cargo audit` is an optional maintainer check provided by `cargo-audit`; it is
not installed or required by either setup script or at runtime.
