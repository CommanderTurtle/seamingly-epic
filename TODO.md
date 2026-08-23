# Production checklist

- [x] Reconstruct the cited ComfyUI seam-fix workflow and tutorial.
- [x] Separate photometric seams from semantic/structural tile errors.
- [x] Choose a single Rust correction engine for the CLI and ComfyUI.
- [ ] Implement lossless PNG decoding/staging with 8-bit and 16-bit support.
- [ ] Preserve alpha, bit depth, color metadata, text metadata, and ComfyUI workflow metadata.
- [ ] Parse explicit X/Y seam coordinates and equal-grid shorthand.
- [ ] Refine nominal grid lines to the strongest nearby persistent boundary step.
- [ ] Estimate robust per-channel log-linear boundary offsets.
- [ ] Solve globally consistent per-tile white-balance/exposure gains.
- [ ] Add confidence gating, correction limits, and a report-only mode.
- [ ] Add smooth local residual correction without spatially filtering image detail.
- [ ] Parallelize analysis/application while keeping PNG encoding bounded in memory.
- [ ] Implement a float32 raw transport for a zero-quantization ComfyUI IMAGE node.
- [ ] Implement a streaming file node for images too large to return as a Comfy tensor.
- [ ] Add setup/build scripts for Windows and Linux.
- [ ] Document exact 2x2 PiD usage for 8192x8192 output.
- [ ] Run formatting, compilation, Clippy, Python compilation, and source audit.
- [ ] Finish focused Git commits and leave the tree clean.

