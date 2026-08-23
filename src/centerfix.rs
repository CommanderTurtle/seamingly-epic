//! Final structural repair from one registered, seam-free center render.
//!
//! This pass is deliberately independent from the canonical photometric and
//! registered-cross passes. The input remains the photometric authority. A
//! star-shaped structural boundary is measured around the supplied center;
//! only the registered reference inside that boundary is gain-matched and
//! composited over the input.

use std::{f64::consts::TAU, fs, path::Path};

use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    color::{Rgb, add, apply_log_gain, log_rgb, norm, scale, sub},
    config::TransferFunction,
    png_io::{PngStage, copy_png_verbatim, validate_output_target},
    report::ImageReport,
    robust::{median, robust_rgb},
};

const TRANSFER: TransferFunction = TransferFunction::Srgb;
const MAX_MATCH_GAIN_STOPS: f64 = 0.75;
const BOUNDARY_SCAN_RADIUS: u32 = 8;
const TANGENT_HALF_WIDTH: i32 = 2;
const GLOBAL_SAMPLE_STRIDE: u32 = 32;
const TARGET_GAIN_SMOOTH_ARC: f64 = 96.0;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CenterReferenceReport {
    pub width: u32,
    pub height: u32,
    pub origin_x: u32,
    pub origin_y: u32,
    pub center_x: u32,
    pub center_y: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RadialBoundaryReport {
    pub minimum_radius: u32,
    pub median_radius: u32,
    pub maximum_radius: u32,
    pub angular_scanlines: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CenterFixReport {
    pub version: u32,
    pub strategy: String,
    pub image: ImageReport,
    pub center: CenterReferenceReport,
    pub boundary: RadialBoundaryReport,
    pub changed_pixels: u64,
    pub total_pixels: u64,
    pub applied: bool,
}

#[derive(Clone, Copy, Debug)]
struct CenterGeometry {
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
    center_x: f64,
    center_y: f64,
}

#[derive(Clone, Copy, Debug)]
struct GainObservation {
    gain: Rgb,
    weight: f64,
}

#[derive(Clone, Copy, Debug)]
struct BandStatistics {
    mean: Rgb,
    dispersion: f64,
    coverage: f64,
}

#[derive(Debug)]
struct CenterPlan {
    geometry: CenterGeometry,
    radii: Vec<f64>,
    boundary_gain: Vec<Rgb>,
    center_gain: Rgb,
}

/// Replace only the unresolved central structure with a registered, seam-free
/// center render. `x` and `y` are the center coordinates, not the reference's
/// top-left corner.
pub fn centerfix_png(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    center: impl AsRef<Path>,
    x: u32,
    y: u32,
    overwrite: bool,
) -> Result<CenterFixReport> {
    let input = input.as_ref();
    let output = output.as_ref();
    let center_path = center.as_ref();
    validate_output_target(input, output, overwrite)?;
    ensure_output_does_not_replace_reference(output, center_path)?;

    let mut base = PngStage::decode(input)?;
    let reference = PngStage::decode(center_path)
        .with_context(|| format!("could not decode --center: {}", center_path.display()))?;
    let (width, height) = base.dimensions();
    let geometry = center_geometry(width, height, x, y, reference.dimensions())?;
    let plan = build_center_plan(&base, &reference, geometry)?;
    let boundary = radial_report(&plan.radii);

    let changed_pixels = base.map_linear_rgb(TRANSFER, 0, |px, py, original| {
        plan.sample(&reference, px, py)
            .map_or(original, |(alpha, corrected)| {
                composite_reference(original, corrected, alpha)
            })
    })?;

    if changed_pixels == 0 {
        copy_png_verbatim(input, output, overwrite)?;
    } else {
        base.encode(output, overwrite)?;
    }

    Ok(CenterFixReport {
        version: 1,
        strategy: "registered_center_radial_structural_boundary_one_way_match".to_owned(),
        image: base.image_report(),
        center: CenterReferenceReport {
            width: geometry.width,
            height: geometry.height,
            origin_x: geometry.origin_x,
            origin_y: geometry.origin_y,
            center_x: x,
            center_y: y,
        },
        boundary,
        changed_pixels,
        total_pixels: u64::from(width) * u64::from(height),
        applied: changed_pixels > 0,
    })
}

fn ensure_output_does_not_replace_reference(output: &Path, reference: &Path) -> Result<()> {
    ensure!(
        output != reference,
        "output path must not replace the center-reference PNG"
    );
    if output.exists() && reference.exists() {
        ensure!(
            fs::canonicalize(output).with_context(|| {
                format!("could not resolve output path: {}", output.display())
            })? != fs::canonicalize(reference).with_context(|| {
                format!(
                    "could not resolve center-reference path: {}",
                    reference.display()
                )
            })?,
            "output resolves to the center-reference PNG"
        );
    }
    Ok(())
}

fn center_geometry(
    base_width: u32,
    base_height: u32,
    center_x: u32,
    center_y: u32,
    reference: (u32, u32),
) -> Result<CenterGeometry> {
    let (width, height) = reference;
    ensure!(
        width == height,
        "--center must be square; received {width}x{height}"
    );
    ensure!(
        width >= 64 && width.is_multiple_of(2),
        "--center dimensions must be an even value of at least 64 pixels"
    );
    ensure!(
        center_x > 0 && center_x < base_width && center_y > 0 && center_y < base_height,
        "--x and --y must lie inside the base image"
    );
    let half = width / 2;
    let origin_x = center_x
        .checked_sub(half)
        .context("--center cannot be centered on --x without extending left of the base")?;
    let origin_y = center_y
        .checked_sub(half)
        .context("--center cannot be centered on --y without extending above the base")?;
    ensure!(
        origin_x + width <= base_width,
        "--center cannot be centered on --x without extending right of the base"
    );
    ensure!(
        origin_y + height <= base_height,
        "--center cannot be centered on --y without extending below the base"
    );
    Ok(CenterGeometry {
        origin_x,
        origin_y,
        width,
        height,
        // Seam coordinates lie between their adjacent pixels. Keeping the
        // half-pixel center makes all four center-adjacent pixels symmetric.
        center_x: f64::from(center_x) - 0.5,
        center_y: f64::from(center_y) - 0.5,
    })
}

fn build_center_plan(
    base: &PngStage,
    reference: &PngStage,
    geometry: CenterGeometry,
) -> Result<CenterPlan> {
    let half_span = geometry.width.min(geometry.height) / 2;
    let (minimum, maximum, step) = radial_search_bounds(half_span)?;
    let angular_scanlines = angular_scanline_count(maximum);
    let center_gain = global_reference_gain(base, reference, geometry);

    let raw_radii: Vec<f64> = (0..angular_scanlines)
        .into_par_iter()
        .map(|index| {
            let angle = angle_at(index, angular_scanlines);
            best_radial_distance(
                base,
                reference,
                geometry,
                angle,
                minimum,
                maximum,
                step,
                center_gain,
            ) as f64
        })
        .collect();
    let radii = smooth_circular_radii(&raw_radii, minimum, maximum);

    let observations: Vec<Option<GainObservation>> = (0..angular_scanlines)
        .into_par_iter()
        .map(|index| {
            radial_boundary_gain(
                base,
                reference,
                geometry,
                angle_at(index, angular_scanlines),
                radii[index].round() as u32,
            )
        })
        .collect();
    let boundary_gain = stabilize_circular_gain_profile(&observations, center_gain, &radii);

    Ok(CenterPlan {
        geometry,
        radii,
        boundary_gain,
        center_gain,
    })
}

fn radial_search_bounds(half_span: u32) -> Result<(u32, u32, u32)> {
    let margin = (half_span / 64).clamp(BOUNDARY_SCAN_RADIUS * 2 + 1, 64);
    let maximum = half_span
        .checked_sub(margin + 1)
        .context("center-reference half-span is too narrow for radial analysis")?;
    let minimum = (half_span / 4).max(8).min(maximum.saturating_sub(1));
    ensure!(minimum < maximum, "center-reference radial search is empty");
    let step = if maximum - minimum >= 1024 { 2 } else { 1 };
    Ok((minimum, maximum, step))
}

fn angular_scanline_count(maximum_radius: u32) -> usize {
    let circumference = (TAU * f64::from(maximum_radius)).ceil() as usize;
    circumference.clamp(2_048, 8_192)
}

fn angle_at(index: usize, count: usize) -> f64 {
    TAU * index as f64 / count as f64
}

#[allow(clippy::too_many_arguments)]
fn best_radial_distance(
    base: &PngStage,
    reference: &PngStage,
    geometry: CenterGeometry,
    angle: f64,
    minimum: u32,
    maximum: u32,
    step: u32,
    reference_gain: Rgb,
) -> u32 {
    let cosine = angle.cos();
    let sine = angle.sin();
    let mut best = minimum;
    let mut best_cost = f64::INFINITY;
    let mut candidate = minimum;
    while candidate <= maximum {
        if let Some(cost) = radial_structural_cost(
            base,
            reference,
            geometry,
            cosine,
            sine,
            candidate,
            reference_gain,
        ) && (cost < best_cost || (cost == best_cost && candidate > best))
        {
            best = candidate;
            best_cost = cost;
        }
        let Some(next) = candidate.checked_add(step) else {
            break;
        };
        candidate = next;
    }
    if step > 1 {
        let refine_start = best.saturating_sub(step - 1).max(minimum);
        let refine_end = (best + step - 1).min(maximum);
        for candidate in refine_start..=refine_end {
            if let Some(cost) = radial_structural_cost(
                base,
                reference,
                geometry,
                cosine,
                sine,
                candidate,
                reference_gain,
            ) && (cost < best_cost || (cost == best_cost && candidate > best))
            {
                best = candidate;
                best_cost = cost;
            }
        }
    }
    best
}

fn radial_structural_cost(
    base: &PngStage,
    reference: &PngStage,
    geometry: CenterGeometry,
    cosine: f64,
    sine: f64,
    radius: u32,
    reference_gain: Rgb,
) -> Option<f64> {
    let mut total = 0.0;
    let mut count = 0_u32;
    for tangent in -TANGENT_HALF_WIDTH..=TANGENT_HALF_WIDTH {
        let (x, y) = radial_coordinate(
            geometry,
            cosine,
            sine,
            f64::from(radius),
            f64::from(tangent),
        )?;
        let local_x = x.checked_sub(geometry.origin_x)?;
        let local_y = y.checked_sub(geometry.origin_y)?;
        if let Some(cost) = structural_cost(base, reference, x, y, local_x, local_y, reference_gain)
        {
            total += cost;
            count += 1;
        }
    }
    (count > 0).then_some(total / f64::from(count))
}

#[allow(clippy::too_many_arguments)]
fn structural_cost(
    base: &PngStage,
    reference: &PngStage,
    base_x: u32,
    base_y: u32,
    reference_x: u32,
    reference_y: u32,
    reference_gain: Rgb,
) -> Option<f64> {
    let base_center = base.linear_rgb_with_transfer(base_x, base_y, TRANSFER)?;
    let reference_center = apply_log_gain(
        reference.linear_rgb_with_transfer(reference_x, reference_y, TRANSFER)?,
        reference_gain,
    );
    let base_feature = log_feature(base_center);
    let reference_feature = log_feature(reference_center);
    let color_cost = norm(sub(base_feature, reference_feature));

    let mut gradient_cost = 0.0;
    let mut gradients = 0_u32;
    for (dx, dy) in [(-1_i32, 0_i32), (1, 0), (0, -1), (0, 1)] {
        let Some(base_neighbor) = offset_sample(base, base_x, base_y, dx, dy) else {
            continue;
        };
        let Some(reference_neighbor) = offset_sample(reference, reference_x, reference_y, dx, dy)
        else {
            continue;
        };
        let reference_neighbor = apply_log_gain(reference_neighbor, reference_gain);
        let base_gradient = sub(log_feature(base_neighbor), base_feature);
        let reference_gradient = sub(log_feature(reference_neighbor), reference_feature);
        gradient_cost += norm(sub(base_gradient, reference_gradient));
        gradients += 1;
    }
    let gradient_cost = if gradients == 0 {
        color_cost
    } else {
        gradient_cost / f64::from(gradients)
    };
    Some(color_cost.mul_add(0.2, gradient_cost * 0.8))
}

fn offset_sample(image: &PngStage, x: u32, y: u32, dx: i32, dy: i32) -> Option<Rgb> {
    let x = x.checked_add_signed(dx)?;
    let y = y.checked_add_signed(dy)?;
    image.linear_rgb_with_transfer(x, y, TRANSFER)
}

fn log_feature(rgb: Rgb) -> Rgb {
    const EPSILON: f64 = 1.0 / 65_535.0;
    rgb.map(|value| (value + EPSILON).ln())
}

fn smooth_circular_radii(raw: &[f64], minimum: u32, maximum: u32) -> Vec<f64> {
    if raw.len() < 3 {
        return raw.to_vec();
    }
    let median_radius = (raw.len() / 768).clamp(5, 24);
    let mut robust = circular_median(raw, median_radius);
    let smooth_radius = (raw.len() / 1536).clamp(4, 16);
    for _ in 0..3 {
        robust = circular_box_smooth_scalar(&robust, smooth_radius);
    }
    robust
        .into_iter()
        .map(|value| value.clamp(f64::from(minimum), f64::from(maximum)))
        .collect()
}

fn circular_median(values: &[f64], radius: usize) -> Vec<f64> {
    (0..values.len())
        .map(|index| {
            let mut window = Vec::with_capacity(radius * 2 + 1);
            for offset in 0..=radius * 2 {
                let wrapped = (index + values.len() + offset - radius) % values.len();
                window.push(values[wrapped]);
            }
            median(&mut window)
        })
        .collect()
}

fn circular_box_smooth_scalar(values: &[f64], radius: usize) -> Vec<f64> {
    (0..values.len())
        .map(|index| {
            let mut sum = 0.0;
            for offset in 0..=radius * 2 {
                let wrapped = (index + values.len() + offset - radius) % values.len();
                sum += values[wrapped];
            }
            sum / (radius * 2 + 1) as f64
        })
        .collect()
}

fn global_reference_gain(base: &PngStage, reference: &PngStage, geometry: CenterGeometry) -> Rgb {
    let mut samples = Vec::new();
    let mut local_y = GLOBAL_SAMPLE_STRIDE / 2;
    while local_y < geometry.height {
        let mut local_x = GLOBAL_SAMPLE_STRIDE / 2;
        while local_x < geometry.width {
            let base_rgb = base.linear_rgb_with_transfer(
                geometry.origin_x + local_x,
                geometry.origin_y + local_y,
                TRANSFER,
            );
            let reference_rgb = reference.linear_rgb_with_transfer(local_x, local_y, TRANSFER);
            if let (Some(base_log), Some(reference_log)) =
                (base_rgb.and_then(log_rgb), reference_rgb.and_then(log_rgb))
            {
                samples.push((sub(base_log, reference_log), 1.0));
            }
            local_x += GLOBAL_SAMPLE_STRIDE;
        }
        local_y += GLOBAL_SAMPLE_STRIDE;
    }
    let limit = MAX_MATCH_GAIN_STOPS * std::f64::consts::LN_2;
    robust_rgb(&samples)
        .map_or([0.0; 3], |estimate| estimate.center)
        .map(|channel| channel.clamp(-limit, limit))
}

fn radial_boundary_gain(
    base: &PngStage,
    reference: &PngStage,
    geometry: CenterGeometry,
    angle: f64,
    radius: u32,
) -> Option<GainObservation> {
    let cosine = angle.cos();
    let sine = angle.sin();
    let band = BOUNDARY_SCAN_RADIUS;
    let outer_far = radial_log_band(
        base,
        reference,
        geometry,
        cosine,
        sine,
        radius.checked_add(band)?,
        radius.checked_add(band * 2)?,
        false,
    )?;
    let outer_near = radial_log_band(
        base,
        reference,
        geometry,
        cosine,
        sine,
        radius,
        radius.checked_add(band)?,
        false,
    )?;
    let inner_near = radial_log_band(
        base,
        reference,
        geometry,
        cosine,
        sine,
        radius.checked_sub(band)?,
        radius,
        true,
    )?;
    let inner_far = radial_log_band(
        base,
        reference,
        geometry,
        cosine,
        sine,
        radius.checked_sub(band * 2)?,
        radius.checked_sub(band)?,
        true,
    )?;
    boundary_gain_observation(outer_far, outer_near, inner_near, inner_far)
}

#[allow(clippy::too_many_arguments)]
fn radial_log_band(
    base: &PngStage,
    reference: &PngStage,
    geometry: CenterGeometry,
    cosine: f64,
    sine: f64,
    start: u32,
    end: u32,
    use_reference: bool,
) -> Option<BandStatistics> {
    if end <= start {
        return None;
    }
    let expected = (end - start) * (TANGENT_HALF_WIDTH as u32 * 2 + 1);
    let mut sum = [0.0; 3];
    let mut values = Vec::with_capacity(expected as usize);
    for radius in start..end {
        for tangent in -TANGENT_HALF_WIDTH..=TANGENT_HALF_WIDTH {
            let Some((x, y)) = radial_coordinate(
                geometry,
                cosine,
                sine,
                f64::from(radius),
                f64::from(tangent),
            ) else {
                continue;
            };
            let rgb = if use_reference {
                let Some(local_x) = x.checked_sub(geometry.origin_x) else {
                    continue;
                };
                let Some(local_y) = y.checked_sub(geometry.origin_y) else {
                    continue;
                };
                reference.linear_rgb_with_transfer(local_x, local_y, TRANSFER)
            } else {
                base.linear_rgb_with_transfer(x, y, TRANSFER)
            };
            if let Some(value) = rgb.and_then(log_rgb) {
                sum = add(sum, value);
                values.push(value);
            }
        }
    }
    if values.len() < expected.div_ceil(2) as usize {
        return None;
    }
    let mean = scale(sum, 1.0 / values.len() as f64);
    let dispersion = values
        .iter()
        .map(|value| norm(sub(*value, mean)))
        .sum::<f64>()
        / values.len() as f64;
    Some(BandStatistics {
        mean,
        dispersion,
        coverage: values.len() as f64 / f64::from(expected),
    })
}

fn boundary_gain_observation(
    outer_far: BandStatistics,
    outer_near: BandStatistics,
    inner_near: BandStatistics,
    inner_far: BandStatistics,
) -> Option<GainObservation> {
    let outer_at_boundary = sub(scale(outer_near.mean, 1.5), scale(outer_far.mean, 0.5));
    let inner_at_boundary = sub(scale(inner_near.mean, 1.5), scale(inner_far.mean, 0.5));
    let texture =
        outer_far.dispersion + outer_near.dispersion + inner_near.dispersion + inner_far.dispersion;
    let coverage = outer_far
        .coverage
        .min(outer_near.coverage)
        .min(inner_near.coverage)
        .min(inner_far.coverage);
    let weight = coverage / (1.0 + (texture / 0.12).powi(2));
    (weight > 0.0).then_some(GainObservation {
        // The input remains authoritative. This gain is applied only to the
        // center reference on the inner side of the detected boundary.
        gain: sub(outer_at_boundary, inner_at_boundary),
        weight,
    })
}

fn stabilize_circular_gain_profile(
    observations: &[Option<GainObservation>],
    fallback: Rgb,
    radii: &[f64],
) -> Vec<Rgb> {
    let samples: Vec<(Rgb, f64)> = observations
        .iter()
        .filter_map(|item| item.map(|observation| (observation.gain, observation.weight)))
        .collect();
    let estimate = robust_rgb(&samples);
    let center = estimate.map_or(fallback, |value| value.center);
    let dispersion = estimate.map_or(0.01, |value| value.dispersion);
    let cutoff = (3.0 * dispersion).max(0.01);
    let raw: Vec<Rgb> = observations
        .iter()
        .map(|item| {
            let Some(observation) = item else {
                return center;
            };
            let residual = sub(observation.gain, center);
            let magnitude = norm(residual);
            let huber = if magnitude <= cutoff {
                1.0
            } else {
                cutoff / magnitude.max(1.0e-12)
            };
            add(
                center,
                scale(residual, huber * observation.weight.sqrt().clamp(0.0, 1.0)),
            )
        })
        .collect();
    let mut radius_values = radii.to_vec();
    let median_radius = median(&mut radius_values).max(1.0);
    let angular_radius = ((TARGET_GAIN_SMOOTH_ARC * observations.len() as f64)
        / (TAU * median_radius))
        .round() as usize;
    let angular_radius = angular_radius.clamp(4, observations.len().saturating_sub(1) / 4);
    let limit = MAX_MATCH_GAIN_STOPS * std::f64::consts::LN_2;
    let mut smoothed = raw;
    for _ in 0..3 {
        smoothed = circular_box_smooth_rgb(&smoothed, angular_radius);
    }
    smoothed
        .into_iter()
        .map(|gain| gain.map(|channel| channel.clamp(-limit, limit)))
        .collect()
}

fn circular_box_smooth_rgb(values: &[Rgb], radius: usize) -> Vec<Rgb> {
    (0..values.len())
        .map(|index| {
            let mut sum = [0.0; 3];
            for offset in 0..=radius * 2 {
                let wrapped = (index + values.len() + offset - radius) % values.len();
                sum = add(sum, values[wrapped]);
            }
            scale(sum, 1.0 / (radius * 2 + 1) as f64)
        })
        .collect()
}

fn radial_coordinate(
    geometry: CenterGeometry,
    cosine: f64,
    sine: f64,
    radius: f64,
    tangent: f64,
) -> Option<(u32, u32)> {
    let x = geometry.center_x + cosine * radius - sine * tangent;
    let y = geometry.center_y + sine * radius + cosine * tangent;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return None;
    }
    Some((x.round() as u32, y.round() as u32))
}

impl CenterPlan {
    fn sample(&self, reference: &PngStage, x: u32, y: u32) -> Option<(f64, Rgb)> {
        if x < self.geometry.origin_x
            || x >= self.geometry.origin_x + self.geometry.width
            || y < self.geometry.origin_y
            || y >= self.geometry.origin_y + self.geometry.height
        {
            return None;
        }
        let dx = f64::from(x) - self.geometry.center_x;
        let dy = f64::from(y) - self.geometry.center_y;
        let distance = dx.hypot(dy);
        let angle = dy.atan2(dx).rem_euclid(TAU);
        let (radius, edge_gain) = circular_interpolate(&self.radii, &self.boundary_gain, angle);
        let alpha = raised_cosine(distance, radius);
        if alpha <= 0.0 {
            return None;
        }
        let local_x = x - self.geometry.origin_x;
        let local_y = y - self.geometry.origin_y;
        let rgb = reference.linear_rgb_with_transfer(local_x, local_y, TRANSFER)?;
        let outward = smooth_step((distance / radius).clamp(0.0, 1.0));
        let gain = interpolate_rgb(self.center_gain, edge_gain, outward);
        Some((alpha, apply_log_gain(rgb, gain)))
    }
}

fn circular_interpolate(radii: &[f64], gains: &[Rgb], angle: f64) -> (f64, Rgb) {
    let position = angle.rem_euclid(TAU) * radii.len() as f64 / TAU;
    let first = position.floor() as usize % radii.len();
    let second = (first + 1) % radii.len();
    let amount = position - position.floor();
    (
        radii[first] + (radii[second] - radii[first]) * amount,
        interpolate_rgb(gains[first], gains[second], amount),
    )
}

fn raised_cosine(distance: f64, extent: f64) -> f64 {
    if extent <= 0.0 || distance >= extent {
        0.0
    } else {
        0.5 * (1.0 + (std::f64::consts::PI * (distance / extent)).cos())
    }
}

fn smooth_step(value: f64) -> f64 {
    value * value * (3.0 - 2.0 * value)
}

fn interpolate_rgb(start: Rgb, end: Rgb, amount: f64) -> Rgb {
    std::array::from_fn(|channel| start[channel] + (end[channel] - start[channel]) * amount)
}

fn composite_reference(base: Rgb, reference: Rgb, alpha: f64) -> Rgb {
    if alpha <= 0.0 {
        return base;
    }
    std::array::from_fn(|channel| {
        if base[channel] >= 1.0 {
            1.0
        } else {
            base[channel]
                .mul_add(1.0 - alpha, reference[channel] * alpha)
                .clamp(0.0, 1.0)
        }
    })
}

fn radial_report(radii: &[f64]) -> RadialBoundaryReport {
    let mut values = radii.to_vec();
    let median_radius = median(&mut values).round() as u32;
    RadialBoundaryReport {
        minimum_radius: radii.iter().copied().fold(f64::INFINITY, f64::min).round() as u32,
        median_radius,
        maximum_radius: radii.iter().copied().fold(0.0_f64, f64::max).round() as u32,
        angular_scanlines: radii.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_center_is_registered_around_the_requested_point() {
        let geometry = center_geometry(8192, 8192, 4096, 4096, (4096, 4096)).unwrap();
        assert_eq!((geometry.origin_x, geometry.origin_y), (2048, 2048));
        assert_eq!((geometry.center_x, geometry.center_y), (4095.5, 4095.5));
    }

    #[test]
    fn standard_center_has_an_unrestricted_radial_search() {
        let (minimum, maximum, step) = radial_search_bounds(2048).unwrap();
        assert_eq!((minimum, maximum, step), (512, 2015, 2));
        assert_eq!(radial_search_bounds(32).unwrap(), (8, 14, 1));
    }

    #[test]
    fn center_weight_is_full_inside_and_exactly_zero_at_the_boundary() {
        assert_eq!(raised_cosine(0.0, 800.0), 1.0);
        assert!((raised_cosine(400.0, 800.0) - 0.5).abs() < 1.0e-12);
        assert_eq!(raised_cosine(800.0, 800.0), 0.0);
    }

    #[test]
    fn boundary_gain_maps_the_inner_reference_to_the_outer_base() {
        let stats = |value: f64| BandStatistics {
            mean: [value.ln(); 3],
            dispersion: 0.0,
            coverage: 1.0,
        };
        let observation =
            boundary_gain_observation(stats(0.5), stats(0.5), stats(0.25), stats(0.25)).unwrap();
        for channel in apply_log_gain([0.25; 3], observation.gain) {
            assert!((channel - 0.5).abs() < 1.0e-12);
        }
    }

    #[test]
    fn circular_interpolation_wraps_without_an_angular_seam() {
        let radii = [10.0, 20.0, 30.0, 40.0];
        let gains = [[0.0; 3], [1.0; 3], [2.0; 3], [3.0; 3]];
        let before = circular_interpolate(&radii, &gains, TAU - 1.0e-9);
        let after = circular_interpolate(&radii, &gains, 1.0e-9);
        assert!((before.0 - after.0).abs() < 1.0e-7);
        assert!(norm(sub(before.1, after.1)) < 1.0e-7);
    }

    #[test]
    fn canonical_base_is_exact_outside_the_center_wave() {
        let base = [0.2, 0.4, 0.6];
        assert_eq!(composite_reference(base, [0.9; 3], 0.0), base);
    }

    #[test]
    fn exact_canonical_white_is_never_darkened() {
        assert_eq!(
            composite_reference([1.0, 0.5, 1.0], [0.1, 0.25, 0.1], 1.0),
            [1.0, 0.25, 1.0]
        );
    }
}
