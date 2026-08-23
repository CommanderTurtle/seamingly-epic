# Seamingly Epic

Seamingly Epic removes straight exposure and white-balance boundaries from
independently refined image tiles. It is designed for very large, lossless PiD
outputs and uses a bounded-memory Rust engine rather than another diffusion pass.

The repository will provide:

- a native PNG CLI for 8K and 16K files;
- an analysis report with detected boundary confidence and proposed gains;
- a ComfyUI IMAGE node backed by the same Rust binary;
- a streaming ComfyUI file node for images too large to keep as tensors.

The implementation is in progress; see [TODO.md](TODO.md) and
[docs/DESIGN.md](docs/DESIGN.md).

