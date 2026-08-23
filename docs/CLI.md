# Native CLI

## Commands

`analyze` decodes and measures the image but never writes corrected pixels. Its
JSON is the safest first look at a new image:

```bash
seamingly-epic analyze input.png --grid 2x2 --report report.json
```

`correct` performs the same analysis, applies accepted corrections, and writes
the output only after encoding succeeds:

```bash
seamingly-epic correct input.png output.png --grid 2x2
```

An existing output is refused unless `--overwrite` is explicit.

`raw-f32` is the machine interface used by the ComfyUI IMAGE node. It consumes
a JSON descriptor for little-endian, contiguous `[B,H,W,C]` float32 files. It is
documented so other local tools can use the same zero-quantization path, but
ordinary users should use the PNG commands.

## Layouts

Grid syntax is columns by rows:

```bash
# One horizontal join
seamingly-epic correct in.png out.png --grid 1x2

# One vertical join
seamingly-epic correct in.png out.png --grid 2x1

# Four quadrants; for 8192x8192 this derives x=4096 and y=4096
seamingly-epic correct in.png out.png --grid 2x2

# Twenty-five equal tiles
seamingly-epic correct in.png out.png --grid 5x5
```

Explicit lists override the corresponding grid axis. To use only explicit
coordinates, disable the grid:

```bash
seamingly-epic correct in.png out.png \
  --no-grid --x-seams 4096 --y-seams 3900,8100
```

Coordinates refer to the assembled output, not the original tile size.

## Settings

| Flag | Default | Purpose |
| --- | ---: | --- |
| `--scan-radius` | 8 | Width of each near/far analysis strip. |
| `--refine-radius` | 2 | Search around a nominal coordinate; use `0` for an exact locked line. |
| `--sample-stride` | 4 | Boundary sampling interval. Lower values inspect more pixels. |
| `--blend-width` | 192 | Raised-cosine distance for the local residual field. |
| `--profile-smooth-radius` | 96 | Low-pass radius along the one-dimensional correction profile. It never blurs source pixels. |
| `--strength` | 1.0 | Global tile-gain multiplier. |
| `--local-strength` | 0.65 | Bounded near-seam residual multiplier. |
| `--max-gain-stops` | 0.75 | Hard per-channel gain limit. |
| `--min-confidence` | 0.18 | Reject boundary segments below this score. |
| `--transfer` | `srgb` | Interpret samples as `srgb` or already `linear`. |
| `--threads` | 0 | Worker count. Zero uses Rayon's platform default. |

The defaults intentionally favor subtle PiD color discontinuities. Increase
`scan-radius` for a very broad illumination offset, not for a split object.
For a known exact concatenate coordinate, `--refine-radius 0` rules out nearby
content edges entirely.

## Report interpretation

Every boundary is reported per tile segment. Important fields are:

- `nominal_coordinate` and `coordinate`: requested line and optional refined
  line.
- `log_jump_rgb` / `jump_stops_rgb`: robust measured right-minus-left or
  bottom-minus-top step.
- `dispersion`: disagreement among along-boundary samples.
- `confidence`: combined coverage, texture reliability, and coherence score.
- `accepted`: whether the segment participates in correction.
- `tile_gains`: globally solved per-tile values when the accepted adjacency
  graph connects every tile.

If accepted constraints do not connect the whole grid, global gains are
disabled. Accepted joins receive only a local fade. This prevents correction of
one isolated pair from creating a new discontinuity against an unmeasured tile.

## Memory and temporary storage

PNG rows are decoded into a temporary memory-mapped raw store, corrected in
parallel, and encoded sequentially. Resident memory is bounded by the operating
system's active mmap pages and encoder buffers; temporary disk demand is about
the uncompressed image size:

- 8192x8192 RGB8: about 192 MiB.
- 8192x8192 RGBA16: about 512 MiB.
- 16384x16384 RGBA16: about 2 GiB.

The Comfy IMAGE transport needs both input and output float32 scratch files.
Set `SEAMINGLY_EPIC_TEMP` to a large local SSD directory when the system temp
volume is unsuitable. The Streaming PNG node is preferable for extreme sizes.

## PNG support

Supported inputs are non-interlaced, non-animated 8/16-bit grayscale, gray
alpha, RGB, and RGBA PNGs. Indexed PNGs and `tRNS` transparency must first be
expanded to RGB/RGBA so transparency remains unambiguous during correction.

Alpha bytes are never changed. Recognized standard PNG color, ICC, EXIF, text,
and ComfyUI metadata chunks are carried into the output. Unknown private chunks
outside the codec's metadata model are not claimed to be preserved.
