use std::{
    borrow::Cow,
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use memmap2::{MmapMut, MmapOptions};
use png::{BitDepth, ColorType, Compression, Info};
use rayon::prelude::*;
use tempfile::{NamedTempFile, tempfile};

use crate::{
    color::{Rgb, apply_log_gain, decode_sample, encode_sample},
    config::{CorrectionConfig, TransferFunction},
    engine::{CorrectionModel, PixelSource, build_model, with_threads},
    report::{CorrectionReport, ImageReport},
};

pub fn analyze_png(input: impl AsRef<Path>, config: &CorrectionConfig) -> Result<CorrectionReport> {
    let stage = PngStage::decode(input.as_ref())?;
    let view = PngView {
        stage: &stage,
        transfer: config.transfer,
    };
    let model = build_model(&view, config, stage.image_report())?;
    Ok(model.report)
}

/// Correct a PNG and atomically place it at `output` after successful encoding.
///
/// When `overwrite` is false, an existing output is never modified.
pub fn correct_png(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    config: &CorrectionConfig,
    overwrite: bool,
) -> Result<CorrectionReport> {
    let input = input.as_ref();
    let output = output.as_ref();
    ensure!(input != output, "input and output paths must be different");
    if output.exists() {
        ensure!(
            fs::canonicalize(input).with_context(|| {
                format!("could not resolve input path: {}", input.display())
            })? != fs::canonicalize(output).with_context(|| {
                format!("could not resolve output path: {}", output.display())
            })?,
            "input and output resolve to the same file"
        );
    }
    if output.exists() && !overwrite {
        bail!(
            "output already exists: {} (pass --overwrite to replace it)",
            output.display()
        );
    }

    let mut stage = PngStage::decode(input)?;
    let model = {
        let view = PngView {
            stage: &stage,
            transfer: config.transfer,
        };
        build_model(&view, config, stage.image_report())?
    };
    if model.report.applied {
        stage.apply(&model, config)?;
    }
    stage.encode(output, overwrite)?;
    Ok(model.report)
}

struct PngStage {
    _file: File,
    pixels: MmapMut,
    info: Info<'static>,
    row_bytes: usize,
    channels: usize,
    bytes_per_sample: usize,
}

impl PngStage {
    fn decode(path: &Path) -> Result<Self> {
        let input = File::open(path)
            .with_context(|| format!("could not open input PNG: {}", path.display()))?;
        let decoder = png::Decoder::new(BufReader::new(input));
        let mut reader = decoder
            .read_info()
            .with_context(|| format!("could not read PNG header: {}", path.display()))?;
        validate_info(reader.info())?;

        let width = reader.info().width;
        let height = reader.info().height;
        let row_bytes = reader
            .output_line_size(width)
            .context("PNG row size exceeds this platform's address space")?;
        let total_bytes = row_bytes
            .checked_mul(height as usize)
            .context("decoded PNG size exceeds this platform's address space")?;
        let file = tempfile().context("could not create temporary pixel store")?;
        file.set_len(total_bytes as u64)
            .context("could not size temporary pixel store")?;
        let mut pixels = map_temporary_file(&file, total_bytes)?;

        for y in 0..height as usize {
            let row = reader
                .next_row()
                .with_context(|| format!("could not decode PNG row {y}"))?
                .context("PNG ended before all declared rows were decoded")?;
            ensure!(
                row.data().len() == row_bytes,
                "decoded PNG row has an unexpected byte count"
            );
            let start = y * row_bytes;
            pixels[start..start + row_bytes].copy_from_slice(row.data());
        }
        ensure!(
            reader
                .next_row()
                .context("could not finish PNG decoding")?
                .is_none(),
            "PNG yielded more rows than declared"
        );
        reader
            .finish()
            .context("could not parse trailing PNG metadata")?;
        let info = owned_info(reader.info());
        let channels = info.color_type.samples();
        let bytes_per_sample = match info.bit_depth {
            BitDepth::Eight => 1,
            BitDepth::Sixteen => 2,
            _ => unreachable!("validated PNG has only 8-bit or 16-bit samples"),
        };

        Ok(Self {
            _file: file,
            pixels,
            info,
            row_bytes,
            channels,
            bytes_per_sample,
        })
    }

    fn image_report(&self) -> ImageReport {
        ImageReport {
            width: self.info.width,
            height: self.info.height,
            channels: self.channels,
            bit_depth: png_depth_label(self.info.color_type, self.info.bit_depth),
            transport: "png-mmap".to_owned(),
        }
    }

    fn apply(&mut self, model: &CorrectionModel, config: &CorrectionConfig) -> Result<()> {
        let row_bytes = self.row_bytes;
        let width = self.info.width;
        let color_type = self.info.color_type;
        let bit_depth = self.info.bit_depth;
        let channels = self.channels;
        let bytes_per_sample = self.bytes_per_sample;
        let transfer = config.transfer;

        with_threads(config.threads, || {
            self.pixels
                .par_chunks_mut(row_bytes)
                .enumerate()
                .for_each(|(y, row)| {
                    for x in 0..width as usize {
                        let pixel_offset = x * channels * bytes_per_sample;
                        if has_alpha(color_type) {
                            let alpha = read_sample(
                                row,
                                pixel_offset + (channels - 1) * bytes_per_sample,
                                bit_depth,
                            );
                            if alpha <= 1.0 / 255.0 {
                                // Preserve hidden RGB as well as alpha. Analysis
                                // excludes the same effectively transparent samples.
                                continue;
                            }
                        }
                        let gain = model.log_gain_at(x as u32, y as u32);
                        if gain.iter().all(|value| value.abs() < 1.0e-15) {
                            continue;
                        }
                        match color_type {
                            ColorType::Grayscale | ColorType::GrayscaleAlpha => {
                                let encoded = read_sample(row, pixel_offset, bit_depth);
                                let linear = decode_sample(encoded, transfer);
                                let gray_gain = (gain[0] + gain[1] + gain[2]) / 3.0;
                                let corrected = apply_log_gain([linear; 3], [gray_gain; 3])[0];
                                write_sample(
                                    row,
                                    pixel_offset,
                                    bit_depth,
                                    encode_sample(corrected, transfer),
                                );
                            }
                            ColorType::Rgb | ColorType::Rgba => {
                                let sample_bytes = bytes_per_sample;
                                let linear = [
                                    decode_sample(
                                        read_sample(row, pixel_offset, bit_depth),
                                        transfer,
                                    ),
                                    decode_sample(
                                        read_sample(row, pixel_offset + sample_bytes, bit_depth),
                                        transfer,
                                    ),
                                    decode_sample(
                                        read_sample(
                                            row,
                                            pixel_offset + 2 * sample_bytes,
                                            bit_depth,
                                        ),
                                        transfer,
                                    ),
                                ];
                                let corrected = apply_log_gain(linear, gain);
                                for (channel, value) in corrected.iter().copied().enumerate() {
                                    write_sample(
                                        row,
                                        pixel_offset + channel * sample_bytes,
                                        bit_depth,
                                        encode_sample(value, transfer),
                                    );
                                }
                            }
                            ColorType::Indexed => unreachable!("indexed PNG was rejected"),
                        }
                    }
                });
            Ok(())
        })?;
        self.pixels
            .flush()
            .context("could not flush corrected pixel store")
    }

    fn encode(&self, output: &Path, overwrite: bool) -> Result<()> {
        let parent = absolute_parent(output)?;
        fs::create_dir_all(&parent)
            .with_context(|| format!("could not create output directory: {}", parent.display()))?;
        let mut temporary = NamedTempFile::new_in(&parent)
            .with_context(|| format!("could not create output beside {}", output.display()))?;
        {
            let writer = BufWriter::new(temporary.as_file_mut());
            let mut encoder = png::Encoder::with_info(writer, self.info.clone())
                .context("source PNG metadata cannot be represented by the encoder")?;
            encoder.set_compression(Compression::Balanced);
            let mut png_writer = encoder
                .write_header()
                .context("could not write PNG header")?;
            let mut stream = png_writer
                .stream_writer_with_size(1024 * 1024)
                .context("could not initialize streaming PNG encoder")?;
            stream
                .write_all(&self.pixels)
                .context("could not encode corrected PNG pixels")?;
            stream.finish().context("could not finish corrected PNG")?;
        }
        temporary
            .as_file_mut()
            .sync_all()
            .context("could not synchronize corrected PNG")?;

        if overwrite && output.exists() {
            fs::remove_file(output).with_context(|| {
                format!("could not replace existing output: {}", output.display())
            })?;
        }
        temporary
            .persist(output)
            .map_err(|error| error.error)
            .with_context(|| format!("could not publish output PNG: {}", output.display()))?;
        Ok(())
    }
}

// PixelSource must use the same transfer function as the requested analysis.
// The stage itself cannot retain a per-call setting, so analysis is wrapped by
// this lightweight view.
struct PngView<'a> {
    stage: &'a PngStage,
    transfer: TransferFunction,
}

impl PixelSource for PngView<'_> {
    fn width(&self) -> u32 {
        self.stage.info.width
    }

    fn height(&self) -> u32 {
        self.stage.info.height
    }

    fn linear_rgb(&self, x: u32, y: u32) -> Option<Rgb> {
        self.stage.linear_rgb_with_transfer(x, y, self.transfer)
    }
}

impl PngStage {
    fn linear_rgb_with_transfer(&self, x: u32, y: u32, transfer: TransferFunction) -> Option<Rgb> {
        if x >= self.info.width || y >= self.info.height {
            return None;
        }
        let pixel_offset =
            y as usize * self.row_bytes + x as usize * self.channels * self.bytes_per_sample;
        if has_alpha(self.info.color_type) {
            let alpha = read_sample(
                &self.pixels,
                pixel_offset + (self.channels - 1) * self.bytes_per_sample,
                self.info.bit_depth,
            );
            if alpha <= 1.0 / 255.0 {
                return None;
            }
        }
        match self.info.color_type {
            ColorType::Grayscale | ColorType::GrayscaleAlpha => {
                let value = decode_sample(
                    read_sample(&self.pixels, pixel_offset, self.info.bit_depth),
                    transfer,
                );
                Some([value; 3])
            }
            ColorType::Rgb | ColorType::Rgba => Some([
                decode_sample(
                    read_sample(&self.pixels, pixel_offset, self.info.bit_depth),
                    transfer,
                ),
                decode_sample(
                    read_sample(
                        &self.pixels,
                        pixel_offset + self.bytes_per_sample,
                        self.info.bit_depth,
                    ),
                    transfer,
                ),
                decode_sample(
                    read_sample(
                        &self.pixels,
                        pixel_offset + 2 * self.bytes_per_sample,
                        self.info.bit_depth,
                    ),
                    transfer,
                ),
            ]),
            ColorType::Indexed => None,
        }
    }
}

fn validate_info(info: &Info<'_>) -> Result<()> {
    ensure!(!info.interlaced, "interlaced PNGs are not supported");
    ensure!(!info.is_animated(), "animated PNGs are not supported");
    ensure!(
        matches!(info.bit_depth, BitDepth::Eight | BitDepth::Sixteen),
        "only 8-bit and 16-bit-per-channel PNG samples are supported"
    );
    ensure!(
        !matches!(info.color_type, ColorType::Indexed),
        "indexed-color PNGs must be converted to RGB or RGBA first"
    );
    ensure!(
        info.trns.is_none(),
        "PNG tRNS transparency is not supported; convert it to an explicit alpha channel first"
    );
    Ok(())
}

#[allow(unsafe_code)]
fn map_temporary_file(file: &File, length: usize) -> Result<MmapMut> {
    // SAFETY: this file is a newly created, exclusively owned temporary store;
    // its length is fixed before mapping and is not changed while the map lives.
    unsafe { MmapOptions::new().len(length).map_mut(file) }
        .context("could not memory-map temporary pixel store")
}

fn read_sample(data: &[u8], offset: usize, depth: BitDepth) -> f64 {
    match depth {
        BitDepth::Eight => f64::from(data[offset]) / 255.0,
        BitDepth::Sixteen => {
            f64::from(u16::from_be_bytes([data[offset], data[offset + 1]])) / 65_535.0
        }
        _ => unreachable!("validated sample depth"),
    }
}

fn write_sample(data: &mut [u8], offset: usize, depth: BitDepth, value: f64) {
    match depth {
        BitDepth::Eight => {
            data[offset] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        BitDepth::Sixteen => {
            let encoded = (value.clamp(0.0, 1.0) * 65_535.0).round() as u16;
            data[offset..offset + 2].copy_from_slice(&encoded.to_be_bytes());
        }
        _ => unreachable!("validated sample depth"),
    }
}

fn has_alpha(color_type: ColorType) -> bool {
    matches!(color_type, ColorType::GrayscaleAlpha | ColorType::Rgba)
}

fn png_depth_label(color_type: ColorType, depth: BitDepth) -> String {
    let channel_bits = match depth {
        BitDepth::Eight => 8,
        BitDepth::Sixteen => 16,
        _ => 0,
    };
    let pixel_bits = color_type.samples() * channel_bits;
    let format = match color_type {
        ColorType::Grayscale => "grayscale",
        ColorType::GrayscaleAlpha => "grayscale+alpha",
        ColorType::Rgb => "RGB",
        ColorType::Rgba => "RGBA",
        ColorType::Indexed => "indexed",
    };
    format!("{pixel_bits}-bit {format} ({channel_bits} bits/channel)")
}

fn owned_info(info: &Info<'_>) -> Info<'static> {
    let mut owned = Info::with_size(info.width, info.height);
    owned.bit_depth = info.bit_depth;
    owned.color_type = info.color_type;
    owned.interlaced = info.interlaced;
    owned.sbit = info.sbit.as_ref().map(|value| Cow::Owned(value.to_vec()));
    owned.trns = info.trns.as_ref().map(|value| Cow::Owned(value.to_vec()));
    owned.pixel_dims = info.pixel_dims;
    owned.palette = info
        .palette
        .as_ref()
        .map(|value| Cow::Owned(value.to_vec()));
    owned.gama_chunk = info.gama_chunk;
    owned.chrm_chunk = info.chrm_chunk;
    owned.bkgd = info.bkgd.as_ref().map(|value| Cow::Owned(value.to_vec()));
    owned.source_gamma = info.source_gamma;
    owned.source_chromaticities = info.source_chromaticities;
    owned.srgb = info.srgb;
    owned.icc_profile = info
        .icc_profile
        .as_ref()
        .map(|value| Cow::Owned(value.to_vec()));
    owned.coding_independent_code_points = info.coding_independent_code_points;
    owned.mastering_display_color_volume = info.mastering_display_color_volume;
    owned.content_light_level = info.content_light_level;
    owned.exif_metadata = info
        .exif_metadata
        .as_ref()
        .map(|value| Cow::Owned(value.to_vec()));
    owned.uncompressed_latin1_text = info.uncompressed_latin1_text.clone();
    owned.compressed_latin1_text = info.compressed_latin1_text.clone();
    owned.utf8_text = info.utf8_text.clone();
    owned
}

fn absolute_parent(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("could not resolve current directory")?
            .join(path)
    };
    absolute
        .parent()
        .map(Path::to_path_buf)
        .context("output path has no parent directory")
}

#[cfg(test)]
mod tests {
    use std::io::BufWriter;

    use super::*;
    use crate::config::{GridSpec, SeamSpec};

    #[test]
    fn round_trips_alpha_and_comfy_text_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.png");
        let output = directory.path().join("output.png");
        let (width, height) = (64_u32, 32_u32);
        let mut source = Vec::with_capacity(width as usize * height as usize * 4);
        let mut alpha = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                let value = if x < width / 2 { 80 } else { 104 };
                let pixel_alpha = ((x + y * 3) % 251 + 4) as u8;
                source.extend_from_slice(&[value, value, value, pixel_alpha]);
                alpha.push(pixel_alpha);
            }
        }
        {
            let file = File::create(&input).unwrap();
            let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
            encoder.set_color(ColorType::Rgba);
            encoder.set_depth(BitDepth::Eight);
            encoder
                .add_text_chunk("workflow".to_owned(), "{\"nodes\":[]}".to_owned())
                .unwrap();
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&source)
                .unwrap();
        }

        let config = CorrectionConfig {
            seams: SeamSpec {
                grid: Some(GridSpec {
                    columns: 2,
                    rows: 1,
                }),
                ..SeamSpec::default()
            },
            scan_radius: 4,
            refine_radius: 0,
            sample_stride: 1,
            blend_width: 0,
            local_strength: 0.0,
            min_confidence: 0.05,
            transfer: TransferFunction::Linear,
            ..CorrectionConfig::default()
        };
        let report = correct_png(&input, &output, &config, false).unwrap();
        assert!(report.applied);

        let file = File::open(&output).unwrap();
        let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let output_info = reader.next_frame(&mut decoded).unwrap();
        decoded.truncate(output_info.buffer_size());
        reader.finish().unwrap();
        assert_eq!(reader.info().bit_depth, BitDepth::Eight);
        assert_eq!(reader.info().color_type, ColorType::Rgba);
        assert!(
            reader
                .info()
                .uncompressed_latin1_text
                .iter()
                .any(|chunk| chunk.keyword == "workflow")
        );
        let output_alpha: Vec<u8> = decoded.chunks_exact(4).map(|pixel| pixel[3]).collect();
        assert_eq!(output_alpha, alpha);
        assert_ne!(decoded, source);
    }

    #[test]
    fn retains_sixteen_bit_samples() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input16.png");
        let output = directory.path().join("output16.png");
        let (width, height) = (32_u32, 16_u32);
        let mut source = Vec::with_capacity(width as usize * height as usize * 6);
        for _y in 0..height {
            for x in 0..width {
                let value = if x < width / 2 {
                    20_000_u16
                } else {
                    24_000_u16
                };
                for _channel in 0..3 {
                    source.extend_from_slice(&value.to_be_bytes());
                }
            }
        }
        {
            let file = File::create(&input).unwrap();
            let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
            encoder.set_color(ColorType::Rgb);
            encoder.set_depth(BitDepth::Sixteen);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&source)
                .unwrap();
        }
        let config = CorrectionConfig {
            seams: SeamSpec {
                grid: Some(GridSpec {
                    columns: 2,
                    rows: 1,
                }),
                ..SeamSpec::default()
            },
            scan_radius: 3,
            refine_radius: 0,
            sample_stride: 1,
            blend_width: 0,
            local_strength: 0.0,
            min_confidence: 0.05,
            transfer: TransferFunction::Linear,
            ..CorrectionConfig::default()
        };
        correct_png(&input, &output, &config, false).unwrap();
        let reader = png::Decoder::new(BufReader::new(File::open(&output).unwrap()))
            .read_info()
            .unwrap();
        assert_eq!(reader.info().bit_depth, BitDepth::Sixteen);
        assert_eq!(reader.info().color_type, ColorType::Rgb);
    }

    #[test]
    fn recognizes_standard_twenty_four_bit_rgb() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("rgb24.png");
        let (width, height) = (32_u32, 16_u32);
        let mut source = Vec::with_capacity(width as usize * height as usize * 3);
        for _y in 0..height {
            for x in 0..width {
                let value = if x < width / 2 { 80_u8 } else { 96_u8 };
                source.extend_from_slice(&[value, value, value]);
            }
        }
        {
            let file = File::create(&input).unwrap();
            let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
            encoder.set_color(ColorType::Rgb);
            encoder.set_depth(BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&source)
                .unwrap();
        }
        let report = analyze_png(
            &input,
            &CorrectionConfig {
                seams: SeamSpec {
                    grid: Some(GridSpec {
                        columns: 2,
                        rows: 1,
                    }),
                    ..SeamSpec::default()
                },
                scan_radius: 3,
                refine_radius: 0,
                sample_stride: 1,
                transfer: TransferFunction::Linear,
                ..CorrectionConfig::default()
            },
        )
        .unwrap();
        assert_eq!(report.image.channels, 3);
        assert_eq!(report.image.bit_depth, "24-bit RGB (8 bits/channel)");
    }
}
