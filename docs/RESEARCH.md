# Research basis

## Target failure

Four independently refined PiD quadrants can preserve geometry and detail while
acquiring slightly different exposure or white balance. When the quadrants are
joined, that nearly uniform photometric offset becomes visible as a perfectly
straight boundary at the grid coordinate. This is not seam carving and does not
require another diffusion pass.

The primary target is a 2048x2048 source divided into four 1024x1024 inputs,
each refined to 4096x4096 and assembled into an 8192x8192 PNG. Its nominal seam
coordinates are `x=4096` and `y=4096`.

## Cited tutorial and workflow

- Rob Adams, [Fixing the Seams from a Tiled Upscaler in ComfyUI](https://www.youtube.com/watch?v=V-ASlpPI87Y), 2024-05-25.
- The video description supplies `SeamFixVer2.1.png`, a 45-node ComfyUI
  workflow with embedded workflow JSON.
- Local research captures are intentionally excluded from Git under
  `.research/`: the auto-caption transcript, embedded JSON, source workflow,
  and selected screenshots.

Notable points:

| Time | Observation |
| --- | --- |
| 01:20-01:36 | Independent tiled refinement produces visible straight seams; reducing denoise also removes wanted detail. |
| 02:18-03:15 | The operator paints over a seam and composites a resized portion of the original image through a grown/blurred mask. |
| 03:38-04:07 | Brightness and saturation are manually adjusted to match the generated output. |
| 04:38-05:54 | A stronger second refinement creates semantic artifacts as well as a stronger boundary. |
| 06:09-07:29 | The mask is expanded and color correction repeated until the join is acceptable. |

The workflow's relevant node chain is:

1. Downscale the refined output for interactive mask painting.
2. Grow and blur the mask.
3. Resize the mask to final resolution.
4. Blur and enlarge the original image.
5. Manually color-correct that reference.
6. Composite the reference patch over the refined output.

That approach is useful for semantic hallucinations, but it is manual and can
replace synthesized detail. Seamingly Epic instead automates the common
photometric case. It will report low confidence rather than pretending that a
model-free color transform can repair incompatible objects or geometry.

## Algorithmic references

- Burt and Adelson, [A Multiresolution Spline With Application to Image Mosaics](https://persci.mit.edu/pub_pdfs/spline83.pdf), 1983. Frequency-dependent transition widths motivate changing only the low-frequency photometric field rather than blurring detail at the boundary.
- Perez, Gangnet, and Blake, [Poisson Image Editing](https://legacy.sites.fas.harvard.edu/~cs278/papers/poisson.pdf), 2003. Gradient-domain constraints motivate preserving local gradients while correcting boundary conditions.
- Levin et al., [Seamless Image Stitching in the Gradient Domain](https://people.csail.mit.edu/alevin/papers/eccv04-blending.pdf), 2004. A seam is usefully measured as a new gradient not supported by either side.
- Kazhdan et al., [Distributed Gradient-Domain Processing of Planar and Spherical Images](https://www.cs.jhu.edu/~misha/MyPapers/ToG10.pdf), 2010. Large stitched images can reconcile values and gradients through a global Poisson system while retaining a cache-aware, parallel implementation.
- Bhat et al., [Fourier Analysis of the 2D Screened Poisson Equation for Gradient Domain Problems](https://grail.cs.washington.edu/projects/screenedPoissonEq/), 2008. Rectangular Neumann problems admit direct cosine-transform solutions rather than an approximate iteration over the full pixel grid.
- [RustDCT](https://docs.rs/rustdct/latest/rustdct/) and [RustFFT](https://docs.rs/rustfft/latest/rustfft/) provide the native DCT-II/III and SIMD-capable FFT kernels used by the full-resolution reconstruction.
- OpenCV's `BlocksChannelsCompensator` documents the established practice of estimating spatially varying per-channel exposure compensation for stitched imagery.
- Current ComfyUI custom-node documentation defines `IMAGE` as a float tensor of shape `[B,H,W,C]`; the native node therefore uses a float32 file transport rather than quantizing through an 8-bit PNG.

## Engineering conclusion

For a persistent straight white-balance boundary, a learned model is unnecessary
and potentially destructive. The production path is a robust Rust scanner with:

- explicit or grid-derived candidate lines;
- robust extrapolation from both sides of each boundary;
- log-linear RGB gain estimates;
- a global least-squares solve over the tile adjacency graph;
- a direct full-resolution Neumann Poisson reconstruction in `f64`;
- exact local closure for non-integrable residuals at seam crossings;
- exact preservation of alpha and untouched spatial detail;
- lossless PNG output with source metadata retained.
