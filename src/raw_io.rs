use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use memmap2::{Mmap, MmapMut, MmapOptions};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    color::{Rgb, apply_log_gain, decode_sample, encode_sample},
    config::CorrectionConfig,
    engine::{CorrectionModel, PixelSource, build_model, with_threads},
    report::{CorrectionReport, ImageReport},
};

/// Descriptor used by the ComfyUI IMAGE transport.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RawF32Descriptor {
    pub input: PathBuf,
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    #[serde(default = "one")]
    pub batch: usize,
    #[serde(default = "three")]
    pub channels: usize,
    #[serde(default)]
    pub config: CorrectionConfig,
    pub report: Option<PathBuf>,
}

const fn one() -> usize {
    1
}

const fn three() -> usize {
    3
}

pub fn correct_raw_f32(descriptor_path: impl AsRef<Path>) -> Result<Vec<CorrectionReport>> {
    let descriptor_path = descriptor_path.as_ref();
    let descriptor_data = fs::read(descriptor_path).with_context(|| {
        format!(
            "could not read float32 descriptor: {}",
            descriptor_path.display()
        )
    })?;
    let mut descriptor: RawF32Descriptor =
        serde_json::from_slice(&descriptor_data).context("float32 descriptor is not valid JSON")?;
    let base = descriptor_path.parent().unwrap_or_else(|| Path::new("."));
    descriptor.input = resolve_path(base, &descriptor.input);
    descriptor.output = resolve_path(base, &descriptor.output);
    descriptor.report = descriptor
        .report
        .as_ref()
        .map(|path| resolve_path(base, path));
    validate_descriptor(&descriptor)?;

    let input_file = File::open(&descriptor.input).with_context(|| {
        format!(
            "could not open float32 input: {}",
            descriptor.input.display()
        )
    })?;
    let expected_bytes = total_bytes(&descriptor)?;
    ensure!(
        input_file
            .metadata()
            .context("could not inspect float32 input")?
            .len()
            == expected_bytes as u64,
        "float32 input byte count does not match descriptor dimensions"
    );
    if let Some(parent) = descriptor.output.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "could not create float32 output directory: {}",
                parent.display()
            )
        })?;
    }
    fs::copy(&descriptor.input, &descriptor.output).with_context(|| {
        format!(
            "could not initialize float32 output: {}",
            descriptor.output.display()
        )
    })?;
    let output_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&descriptor.output)
        .with_context(|| {
            format!(
                "could not open float32 output: {}",
                descriptor.output.display()
            )
        })?;
    let input_map = map_read_only(&input_file, expected_bytes)?;
    let mut output_map = map_read_write(&output_file, expected_bytes)?;
    let image_bytes = image_bytes(&descriptor)?;
    let mut reports = Vec::with_capacity(descriptor.batch);

    for batch_index in 0..descriptor.batch {
        let start = batch_index * image_bytes;
        let end = start + image_bytes;
        let view = RawView {
            data: &input_map[start..end],
            width: descriptor.width,
            height: descriptor.height,
            channels: descriptor.channels,
            transfer: descriptor.config.transfer,
        };
        let model = build_model(
            &view,
            &descriptor.config,
            ImageReport {
                width: descriptor.width,
                height: descriptor.height,
                channels: descriptor.channels,
                bit_depth: "f32".to_owned(),
                transport: format!("raw-f32-batch-{batch_index}"),
            },
        )?;
        if model.report.applied {
            apply_raw(
                &mut output_map[start..end],
                &model,
                &descriptor.config,
                descriptor.width,
                descriptor.height,
                descriptor.channels,
            )?;
        }
        reports.push(model.report);
    }
    output_map
        .flush()
        .context("could not flush float32 output")?;

    if let Some(report_path) = &descriptor.report {
        if let Some(parent) = report_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("could not create report directory: {}", parent.display())
            })?;
        }
        let report_data = serde_json::to_vec_pretty(&reports)
            .context("could not serialize float32 correction reports")?;
        fs::write(report_path, report_data)
            .with_context(|| format!("could not write report: {}", report_path.display()))?;
    }
    Ok(reports)
}

fn validate_descriptor(descriptor: &RawF32Descriptor) -> Result<()> {
    ensure!(
        descriptor.width >= 4,
        "float32 image width must be at least four"
    );
    ensure!(
        descriptor.height >= 4,
        "float32 image height must be at least four"
    );
    ensure!(descriptor.batch > 0, "float32 batch must be non-zero");
    ensure!(
        matches!(descriptor.channels, 3 | 4),
        "float32 transport supports RGB or RGBA tensors only"
    );
    ensure!(
        descriptor.input != descriptor.output,
        "float32 input and output paths must be different"
    );
    Ok(())
}

fn total_bytes(descriptor: &RawF32Descriptor) -> Result<usize> {
    image_bytes(descriptor)?
        .checked_mul(descriptor.batch)
        .context("float32 batch exceeds this platform's address space")
}

fn image_bytes(descriptor: &RawF32Descriptor) -> Result<usize> {
    (descriptor.width as usize)
        .checked_mul(descriptor.height as usize)
        .and_then(|value| value.checked_mul(descriptor.channels))
        .and_then(|value| value.checked_mul(size_of::<f32>()))
        .context("float32 image exceeds this platform's address space")
}

struct RawView<'a> {
    data: &'a [u8],
    width: u32,
    height: u32,
    channels: usize,
    transfer: crate::config::TransferFunction,
}

impl PixelSource for RawView<'_> {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn linear_rgb(&self, x: u32, y: u32) -> Option<Rgb> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let sample = (y as usize * self.width as usize + x as usize) * self.channels;
        if self.channels == 4 && read_f32(self.data, sample + 3) <= 1.0 / 255.0 {
            return None;
        }
        Some([
            decode_sample(f64::from(read_f32(self.data, sample)), self.transfer),
            decode_sample(f64::from(read_f32(self.data, sample + 1)), self.transfer),
            decode_sample(f64::from(read_f32(self.data, sample + 2)), self.transfer),
        ])
    }
}

fn apply_raw(
    data: &mut [u8],
    model: &CorrectionModel,
    config: &CorrectionConfig,
    width: u32,
    height: u32,
    channels: usize,
) -> Result<()> {
    let row_bytes = width as usize * channels * size_of::<f32>();
    ensure!(
        data.len() == row_bytes * height as usize,
        "float32 output slice has an unexpected length"
    );
    with_threads(config.threads, || {
        data.par_chunks_mut(row_bytes)
            .enumerate()
            .for_each(|(y, row)| {
                for x in 0..width as usize {
                    let gain = model.log_gain_at(x as u32, y as u32);
                    if gain.iter().all(|value| value.abs() < 1.0e-15) {
                        continue;
                    }
                    let sample = x * channels;
                    let linear = [
                        decode_sample(f64::from(read_f32(row, sample)), config.transfer),
                        decode_sample(f64::from(read_f32(row, sample + 1)), config.transfer),
                        decode_sample(f64::from(read_f32(row, sample + 2)), config.transfer),
                    ];
                    let corrected = apply_log_gain(linear, gain);
                    for (channel, value) in corrected.iter().copied().enumerate() {
                        write_f32(
                            row,
                            sample + channel,
                            encode_sample(value, config.transfer) as f32,
                        );
                    }
                }
            });
        Ok(())
    })
}

fn read_f32(data: &[u8], sample: usize) -> f32 {
    let offset = sample * size_of::<f32>();
    f32::from_le_bytes(
        data[offset..offset + size_of::<f32>()]
            .try_into()
            .expect("validated float32 byte range"),
    )
}

fn write_f32(data: &mut [u8], sample: usize, value: f32) {
    let offset = sample * size_of::<f32>();
    data[offset..offset + size_of::<f32>()].copy_from_slice(&value.to_le_bytes());
}

#[allow(unsafe_code)]
fn map_read_only(file: &File, length: usize) -> Result<Mmap> {
    // SAFETY: the input is held open, has a validated fixed length, and is not
    // mutated by this process while the read-only mapping exists.
    unsafe { MmapOptions::new().len(length).map(file) }
        .context("could not memory-map float32 input")
}

#[allow(unsafe_code)]
fn map_read_write(file: &File, length: usize) -> Result<MmapMut> {
    // SAFETY: the output file is exclusively managed by this call, has a fixed
    // validated length, and remains open for the entire mapping lifetime.
    unsafe { MmapOptions::new().len(length).map_mut(file) }
        .context("could not memory-map float32 output")
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GridSpec, SeamSpec, TransferFunction};

    #[test]
    fn corrects_float32_batches_without_touching_alpha() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.f32");
        let output = directory.path().join("output.f32");
        let report = directory.path().join("report.json");
        let descriptor_path = directory.path().join("descriptor.json");
        let (width, height, channels) = (64_u32, 32_u32, 4_usize);
        let mut bytes = Vec::new();
        for _y in 0..height {
            for x in 0..width {
                let value = if x < width / 2 { 0.30_f32 } else { 0.39_f32 };
                for channel in [value, value, value, 0.75_f32] {
                    bytes.extend_from_slice(&channel.to_le_bytes());
                }
            }
        }
        fs::write(&input, &bytes).unwrap();
        let descriptor = RawF32Descriptor {
            input: input.clone(),
            output: output.clone(),
            width,
            height,
            batch: 1,
            channels,
            config: CorrectionConfig {
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
            },
            report: Some(report.clone()),
        };
        fs::write(
            &descriptor_path,
            serde_json::to_vec_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let reports = correct_raw_f32(&descriptor_path).unwrap();
        assert!(reports[0].applied);
        assert!(report.is_file());
        let corrected = fs::read(output).unwrap();
        assert_ne!(corrected, bytes);
        for (original, result) in bytes
            .chunks_exact(channels * size_of::<f32>())
            .zip(corrected.chunks_exact(channels * size_of::<f32>()))
        {
            let alpha_offset = 3 * size_of::<f32>();
            assert_eq!(
                &original[alpha_offset..alpha_offset + size_of::<f32>()],
                &result[alpha_offset..alpha_offset + size_of::<f32>()]
            );
        }
    }
}
