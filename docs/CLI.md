# Native CLI

## Direct interface

For a known concatenate line, no subcommand or grid declaration is needed:

```bash
seamingly-epic --x 4096 --y 4096 --in myfile.png --out fixed.png
```

Comma-separated lines dynamically define the regions. Two X lines and one Y
line form a 3x2 adjacency graph with six regions:

```bash
seamingly-epic --x 3084,5887 --y 4096 --in myfile.png --out fixed.png
```

There is no separate grid or tuning configuration. Four X lines and seven Y
lines automatically become five columns by eight rows, for example:

```bash
seamingly-epic --x 222,3333,7755,8842 \
  --y 123,1234,2222,4444,5555,6666,7777 \
  --in myfile.png --out output.png
```

That command derives 40 tiles. Its four vertical lines are each split across
eight row regions, and its seven horizontal lines are each split across five
column regions: `4*8 + 7*5 = 67` distinct shared-edge measurements.

Each vertical line is scanwalked at every Y position within every row region,
and each horizontal line at every X position within every column region. All
accepted neighboring deltas are solved together. The resulting position-varying
profiles become smooth per-pixel correction fields on both sides of every join.
Direct coordinates are exact and do not search nearby pixels. Use the advanced
`correct` subcommand when coordinate refinement or tuning is intentionally
wanted.

Only genuinely shared edges are measured directly. Diagonal and distant tiles
do not provide a trustworthy common scanline, so their relationship is carried
through every independent path in the connected tile graph. Solving the entire
weighted graph simultaneously is the algebraic equivalent of expanding
cumulative adjacency rings until every tile is included. Thus a corner tile's
gain depends on the opposite corner without pretending those corners share raw
pixels.

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
| `--refine-radius` | 0 | Keep the supplied concatenate line exact; increase only when an earlier crop/resize may have shifted it. |
| `--sample-stride` | 1 | Boundary sampling interval. The default inspects every row/column along every segment. |
| `--blend-width` | 192 | Raised-cosine distance for the local residual field. |
| `--profile-smooth-radius` | 96 | Low-pass radius along the one-dimensional correction profile. It never blurs source pixels. |
| `--strength` | 1.0 | Global tile-gain multiplier. |
| `--local-strength` | 1.0 | Bounded position-varying residual multiplier. |
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

Supported inputs are non-interlaced, non-animated grayscale, gray-alpha, RGB,
and RGBA PNGs with 8- or 16-bit samples per channel. In conventional total-pixel
terminology this includes RGB24, RGBA32, RGB48, and RGBA64. Indexed PNGs and
`tRNS` transparency must first be expanded to RGB/RGBA so transparency remains
unambiguous during correction.

Alpha bytes are never changed. Recognized standard PNG color, ICC, EXIF, text,
and ComfyUI metadata chunks are carried into the output. Unknown private chunks
outside the codec's metadata model are not claimed to be preserved.
