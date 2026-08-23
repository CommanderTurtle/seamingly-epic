//! Structural seam replacement from two registered overlap renders.
//!
//! The portrait reference crosses the base image's vertical join without a
//! vertical tile boundary. The landscape reference does the analogous job for
//! the horizontal join. Four unrestricted structural-match curves define an
//! irregular cross. At each curve, the reference's inner bands are matched to
//! the authoritative base's outer bands before a raised-cosine alpha wave is
//! evaluated from the original join to that curve.

use std::{f64::consts::PI, fs, path::Path};

use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    color::{Rgb, add, apply_log_gain, log_rgb, norm, scale, sub},
    config::TransferFunction,
    png_io::{PngStage, copy_png_verbatim, validate_output_target},
    report::ImageReport,
    robust::{median, robust_rgb, smooth_profile},
};

const TRANSFER: TransferFunction = TransferFunction::Srgb;
const MAX_MATCH_GAIN_STOPS: f64 = 0.75;
const BOUNDARY_SCAN_RADIUS: u32 = 8;
const GAIN_PROFILE_SMOOTH_RADIUS: usize = 96;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CrossReferenceReport {
    pub width: u32,
    pub height: u32,
    pub origin_x: u32,
    pub origin_y: u32,
    pub repairs: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StitchRangeReport {
    pub minimum_distance: u32,
    pub median_distance: u32,
    pub maximum_distance: u32,
    pub scanlines: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StructuralReport {
    pub version: u32,
    pub strategy: String,
    pub image: ImageReport,
    pub x: u32,
    pub y: u32,
    pub xcross: CrossReferenceReport,
    pub ycross: CrossReferenceReport,
    pub left_stitch: StitchRangeReport,
    pub right_stitch: StitchRangeReport,
    pub top_stitch: StitchRangeReport,
    pub bottom_stitch: StitchRangeReport,
    pub intersection_strategy: String,
    pub changed_pixels: u64,
    pub total_pixels: u64,
    pub applied: bool,
}

#[derive(Clone, Copy, Debug)]
enum Side {
    Negative,
    Positive,
}

#[derive(Clone, Copy, Debug)]
struct CrossGeometry {
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct VerticalPlan {
    geometry: CrossGeometry,
    seam: u32,
    left_distance: Vec<f64>,
    right_distance: Vec<f64>,
    left_gain: Vec<Rgb>,
    right_gain: Vec<Rgb>,
    center_gain: Vec<Rgb>,
}

#[derive(Debug)]
struct HorizontalPlan {
    geometry: CrossGeometry,
    seam: u32,
    top_distance: Vec<f64>,
    bottom_distance: Vec<f64>,
    top_gain: Vec<Rgb>,
    bottom_gain: Vec<Rgb>,
    center_gain: Vec<Rgb>,
}

/// Replace the structural content around one vertical and one horizontal join
/// with two registered overlap renders. No resizing or geometric registration
/// is performed: invalid cross dimensions are rejected rather than guessed.
pub fn strucfix_png(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    xcross: impl AsRef<Path>,
    ycross: impl AsRef<Path>,
    x: u32,
    y: u32,
    overwrite: bool,
) -> Result<StructuralReport> {
    let input = input.as_ref();
    let output = output.as_ref();
    let xcross_path = xcross.as_ref();
    let ycross_path = ycross.as_ref();
    validate_output_target(input, output, overwrite)?;
    ensure_output_does_not_replace_reference(output, xcross_path)?;
    ensure_output_does_not_replace_reference(output, ycross_path)?;

    let mut base = PngStage::decode(input)?;
    let x_reference = PngStage::decode(xcross_path)
        .with_context(|| format!("could not decode --xcross: {}", xcross_path.display()))?;
    let y_reference = PngStage::decode(ycross_path)
        .with_context(|| format!("could not decode --ycross: {}", ycross_path.display()))?;
    let (width, height) = base.dimensions();
    ensure!(x > 0 && x < width, "--x must lie inside the base image");
    ensure!(y > 0 && y < height, "--y must lie inside the base image");

    let x_geometry = landscape_geometry(width, height, y, x_reference.dimensions())?;
    let y_geometry = portrait_geometry(width, height, x, y_reference.dimensions())?;
    let vertical = build_vertical_plan(&base, &y_reference, y_geometry, x)?;
    let horizontal = build_horizontal_plan(&base, &x_reference, x_geometry, y)?;

    let left_stitch = range_report(&vertical.left_distance);
    let right_stitch = range_report(&vertical.right_distance);
    let top_stitch = range_report(&horizontal.top_distance);
    let bottom_stitch = range_report(&horizontal.bottom_distance);

    let changed_pixels = base.map_linear_rgb(TRANSFER, 0, |px, py, original| {
        let vertical_sample = vertical.sample(&y_reference, px, py);
        let horizontal_sample = horizontal.sample(&x_reference, px, py);
        combine_references(original, vertical_sample, horizontal_sample)
    })?;

    if changed_pixels == 0 {
        copy_png_verbatim(input, output, overwrite)?;
    } else {
        base.encode(output, overwrite)?;
    }

    Ok(StructuralReport {
        version: 3,
        strategy: "registered_cross_unrestricted_boundary_matched_wave".to_owned(),
        image: base.image_report(),
        x,
        y,
        xcross: CrossReferenceReport {
            width: x_geometry.width,
            height: x_geometry.height,
            origin_x: x_geometry.origin_x,
            origin_y: x_geometry.origin_y,
            repairs: "horizontal seam (--y)".to_owned(),
        },
        ycross: CrossReferenceReport {
            width: y_geometry.width,
            height: y_geometry.height,
            origin_x: y_geometry.origin_x,
            origin_y: y_geometry.origin_y,
            repairs: "vertical seam (--x)".to_owned(),
        },
        left_stitch,
        right_stitch,
        top_stitch,
        bottom_stitch,
        intersection_strategy: "smooth union opacity with orthogonal-seam-aware axis weights"
            .to_owned(),
        changed_pixels,
        total_pixels: u64::from(width) * u64::from(height),
        applied: changed_pixels > 0,
    })
}

fn ensure_output_does_not_replace_reference(output: &Path, reference: &Path) -> Result<()> {
    ensure!(
        output != reference,
        "output path must not replace a cross-reference PNG"
    );
    if output.exists() && reference.exists() {
        ensure!(
            fs::canonicalize(output).with_context(|| {
                format!("could not resolve output path: {}", output.display())
            })? != fs::canonicalize(reference).with_context(|| {
                format!("could not resolve reference path: {}", reference.display())
            })?,
            "output resolves to a cross-reference PNG"
        );
    }
    Ok(())
}

fn landscape_geometry(
    base_width: u32,
    base_height: u32,
    seam_y: u32,
    reference: (u32, u32),
) -> Result<CrossGeometry> {
    let (width, height) = reference;
    ensure!(
        width == base_width,
        "--xcross must span the base width exactly ({base_width}px); received {width}px"
    );
    ensure!(
        height >= 32 && height.is_multiple_of(2),
        "--xcross height must be an even value of at least 32 pixels"
    );
    let half = height / 2;
    let origin_y = seam_y
        .checked_sub(half)
        .context("--xcross cannot be centered on --y without extending above the base")?;
    ensure!(
        origin_y + height <= base_height,
        "--xcross cannot be centered on --y without extending below the base"
    );
    Ok(CrossGeometry {
        origin_x: 0,
        origin_y,
        width,
        height,
    })
}

fn portrait_geometry(
    base_width: u32,
    base_height: u32,
    seam_x: u32,
    reference: (u32, u32),
) -> Result<CrossGeometry> {
    let (width, height) = reference;
    ensure!(
        height == base_height,
        "--ycross must span the base height exactly ({base_height}px); received {height}px"
    );
    ensure!(
        width >= 32 && width.is_multiple_of(2),
        "--ycross width must be an even value of at least 32 pixels"
    );
    let half = width / 2;
    let origin_x = seam_x
        .checked_sub(half)
        .context("--ycross cannot be centered on --x without extending left of the base")?;
    ensure!(
        origin_x + width <= base_width,
        "--ycross cannot be centered on --x without extending right of the base"
    );
    Ok(CrossGeometry {
        origin_x,
        origin_y: 0,
        width,
        height,
    })
}

fn build_vertical_plan(
    base: &PngStage,
    reference: &PngStage,
    geometry: CrossGeometry,
    seam: u32,
) -> Result<VerticalPlan> {
    let half = geometry.width / 2;
    ensure!(
        geometry.origin_x + half == seam,
        "--ycross is not centered exactly on --x"
    );
    let (minimum, maximum, step) = match_search_bounds(half)?;
    let height = geometry.height;
    let raw_left: Vec<f64> = (0..height)
        .into_par_iter()
        .map(|y| {
            best_vertical_distance(
                base,
                reference,
                geometry,
                seam,
                y,
                Side::Negative,
                minimum,
                maximum,
                step,
            ) as f64
        })
        .collect();
    let raw_right: Vec<f64> = (0..height)
        .into_par_iter()
        .map(|y| {
            best_vertical_distance(
                base,
                reference,
                geometry,
                seam,
                y,
                Side::Positive,
                minimum,
                maximum,
                step,
            ) as f64
        })
        .collect();
    let left_distance = smooth_distances(&raw_left, minimum, maximum);
    let right_distance = smooth_distances(&raw_right, minimum, maximum);
    let left_observations: Vec<Option<GainObservation>> = (0..height)
        .into_par_iter()
        .map(|y| {
            vertical_boundary_gain(
                base,
                reference,
                geometry,
                seam,
                y,
                Side::Negative,
                left_distance[y as usize].round() as u32,
            )
        })
        .collect();
    let right_observations: Vec<Option<GainObservation>> = (0..height)
        .into_par_iter()
        .map(|y| {
            vertical_boundary_gain(
                base,
                reference,
                geometry,
                seam,
                y,
                Side::Positive,
                right_distance[y as usize].round() as u32,
            )
        })
        .collect();
    let (left_gain, right_gain) = matched_gain_profiles(&left_observations, &right_observations);
    let center_gain = center_gain_profile(&left_gain, &right_gain, &left_distance, &right_distance);
    Ok(VerticalPlan {
        geometry,
        seam,
        left_distance,
        right_distance,
        left_gain,
        right_gain,
        center_gain,
    })
}

fn build_horizontal_plan(
    base: &PngStage,
    reference: &PngStage,
    geometry: CrossGeometry,
    seam: u32,
) -> Result<HorizontalPlan> {
    let half = geometry.height / 2;
    ensure!(
        geometry.origin_y + half == seam,
        "--xcross is not centered exactly on --y"
    );
    let (minimum, maximum, step) = match_search_bounds(half)?;
    let width = geometry.width;
    let raw_top: Vec<f64> = (0..width)
        .into_par_iter()
        .map(|x| {
            best_horizontal_distance(
                base,
                reference,
                geometry,
                seam,
                x,
                Side::Negative,
                minimum,
                maximum,
                step,
            ) as f64
        })
        .collect();
    let raw_bottom: Vec<f64> = (0..width)
        .into_par_iter()
        .map(|x| {
            best_horizontal_distance(
                base,
                reference,
                geometry,
                seam,
                x,
                Side::Positive,
                minimum,
                maximum,
                step,
            ) as f64
        })
        .collect();
    let top_distance = smooth_distances(&raw_top, minimum, maximum);
    let bottom_distance = smooth_distances(&raw_bottom, minimum, maximum);
    let top_observations: Vec<Option<GainObservation>> = (0..width)
        .into_par_iter()
        .map(|x| {
            horizontal_boundary_gain(
                base,
                reference,
                geometry,
                seam,
                x,
                Side::Negative,
                top_distance[x as usize].round() as u32,
            )
        })
        .collect();
    let bottom_observations: Vec<Option<GainObservation>> = (0..width)
        .into_par_iter()
        .map(|x| {
            horizontal_boundary_gain(
                base,
                reference,
                geometry,
                seam,
                x,
                Side::Positive,
                bottom_distance[x as usize].round() as u32,
            )
        })
        .collect();
    let (top_gain, bottom_gain) = matched_gain_profiles(&top_observations, &bottom_observations);
    let center_gain = center_gain_profile(&top_gain, &bottom_gain, &top_distance, &bottom_distance);
    Ok(HorizontalPlan {
        geometry,
        seam,
        top_distance,
        bottom_distance,
        top_gain,
        bottom_gain,
        center_gain,
    })
}

fn match_search_bounds(half_span: u32) -> Result<(u32, u32, u32)> {
    let margin = (half_span / 64).clamp(4, 64);
    let maximum = half_span
        .checked_sub(margin + 1)
        .context("cross-reference half-span is too narrow for a stitch search")?;
    let minimum = (half_span / 4).max(8).min(maximum);
    ensure!(minimum < maximum, "cross-reference stitch search is empty");
    let step = if maximum - minimum >= 1024 { 2 } else { 1 };
    Ok((minimum, maximum, step))
}

#[allow(clippy::too_many_arguments)]
fn best_vertical_distance(
    base: &PngStage,
    reference: &PngStage,
    geometry: CrossGeometry,
    seam: u32,
    y: u32,
    side: Side,
    minimum: u32,
    maximum: u32,
    step: u32,
) -> u32 {
    best_distance(minimum, maximum, step, |distance| {
        let x = match side {
            Side::Negative => seam - 1 - distance,
            Side::Positive => seam + distance,
        };
        structural_cost(
            base,
            reference,
            x,
            y,
            x - geometry.origin_x,
            y - geometry.origin_y,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn best_horizontal_distance(
    base: &PngStage,
    reference: &PngStage,
    geometry: CrossGeometry,
    seam: u32,
    x: u32,
    side: Side,
    minimum: u32,
    maximum: u32,
    step: u32,
) -> u32 {
    best_distance(minimum, maximum, step, |distance| {
        let y = match side {
            Side::Negative => seam - 1 - distance,
            Side::Positive => seam + distance,
        };
        structural_cost(
            base,
            reference,
            x,
            y,
            x - geometry.origin_x,
            y - geometry.origin_y,
        )
    })
}

fn best_distance<F>(minimum: u32, maximum: u32, step: u32, cost: F) -> u32
where
    F: Fn(u32) -> Option<f64>,
{
    let mut best = minimum;
    let mut best_cost = f64::INFINITY;
    let mut candidate = minimum;
    while candidate <= maximum {
        if let Some(value) = cost(candidate)
            && (value < best_cost || (value == best_cost && candidate > best))
        {
            best = candidate;
            best_cost = value;
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
            if let Some(value) = cost(candidate)
                && (value < best_cost || (value == best_cost && candidate > best))
            {
                best = candidate;
                best_cost = value;
            }
        }
    }
    best
}

fn structural_cost(
    base: &PngStage,
    reference: &PngStage,
    base_x: u32,
    base_y: u32,
    reference_x: u32,
    reference_y: u32,
) -> Option<f64> {
    let base_center = base.linear_rgb_with_transfer(base_x, base_y, TRANSFER)?;
    let reference_center =
        reference.linear_rgb_with_transfer(reference_x, reference_y, TRANSFER)?;
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

fn smooth_distances(raw: &[f64], minimum: u32, maximum: u32) -> Vec<f64> {
    if raw.len() < 3 {
        return raw.to_vec();
    }
    let median_radius = (raw.len() / 512).clamp(4, 24);
    let mut robust = Vec::with_capacity(raw.len());
    for index in 0..raw.len() {
        let start = index.saturating_sub(median_radius);
        let end = (index + median_radius + 1).min(raw.len());
        let mut window = raw[start..end].to_vec();
        robust.push(median(&mut window));
    }
    let smooth_radius = (raw.len() / 1024).clamp(4, 16);
    for _ in 0..3 {
        robust = box_smooth_scalar(&robust, smooth_radius);
    }
    robust
        .into_iter()
        .map(|value| value.clamp(f64::from(minimum), f64::from(maximum)))
        .collect()
}

fn box_smooth_scalar(values: &[f64], radius: usize) -> Vec<f64> {
    let mut prefix = vec![0.0; values.len() + 1];
    for (index, value) in values.iter().enumerate() {
        prefix[index + 1] = prefix[index] + value;
    }
    (0..values.len())
        .map(|index| {
            let start = index.saturating_sub(radius);
            let end = (index + radius + 1).min(values.len());
            (prefix[end] - prefix[start]) / (end - start) as f64
        })
        .collect()
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

fn vertical_boundary_gain(
    base: &PngStage,
    reference: &PngStage,
    geometry: CrossGeometry,
    seam: u32,
    y: u32,
    side: Side,
    distance: u32,
) -> Option<GainObservation> {
    let split = match side {
        Side::Negative => seam.checked_sub(distance)?,
        Side::Positive => seam.checked_add(distance)?,
    };
    let ranges = boundary_ranges(split, side, BOUNDARY_SCAN_RADIUS)?;
    let reference_y = y.checked_sub(geometry.origin_y)?;
    let outer_far = log_band(ranges[0].0, ranges[0].1, |axis| {
        base.linear_rgb_with_transfer(axis, y, TRANSFER)
    })?;
    let outer_near = log_band(ranges[1].0, ranges[1].1, |axis| {
        base.linear_rgb_with_transfer(axis, y, TRANSFER)
    })?;
    let inner_near = log_band(ranges[2].0, ranges[2].1, |axis| {
        axis.checked_sub(geometry.origin_x)
            .and_then(|local_x| reference.linear_rgb_with_transfer(local_x, reference_y, TRANSFER))
    })?;
    let inner_far = log_band(ranges[3].0, ranges[3].1, |axis| {
        axis.checked_sub(geometry.origin_x)
            .and_then(|local_x| reference.linear_rgb_with_transfer(local_x, reference_y, TRANSFER))
    })?;
    boundary_gain_observation(outer_far, outer_near, inner_near, inner_far)
}

fn horizontal_boundary_gain(
    base: &PngStage,
    reference: &PngStage,
    geometry: CrossGeometry,
    seam: u32,
    x: u32,
    side: Side,
    distance: u32,
) -> Option<GainObservation> {
    let split = match side {
        Side::Negative => seam.checked_sub(distance)?,
        Side::Positive => seam.checked_add(distance)?,
    };
    let ranges = boundary_ranges(split, side, BOUNDARY_SCAN_RADIUS)?;
    let reference_x = x.checked_sub(geometry.origin_x)?;
    let outer_far = log_band(ranges[0].0, ranges[0].1, |axis| {
        base.linear_rgb_with_transfer(x, axis, TRANSFER)
    })?;
    let outer_near = log_band(ranges[1].0, ranges[1].1, |axis| {
        base.linear_rgb_with_transfer(x, axis, TRANSFER)
    })?;
    let inner_near = log_band(ranges[2].0, ranges[2].1, |axis| {
        axis.checked_sub(geometry.origin_y)
            .and_then(|local_y| reference.linear_rgb_with_transfer(reference_x, local_y, TRANSFER))
    })?;
    let inner_far = log_band(ranges[3].0, ranges[3].1, |axis| {
        axis.checked_sub(geometry.origin_y)
            .and_then(|local_y| reference.linear_rgb_with_transfer(reference_x, local_y, TRANSFER))
    })?;
    boundary_gain_observation(outer_far, outer_near, inner_near, inner_far)
}

fn boundary_ranges(split: u32, side: Side, radius: u32) -> Option<[(u32, u32); 4]> {
    let twice = radius.checked_mul(2)?;
    let before_far = split.checked_sub(twice)?;
    let before_near = split.checked_sub(radius)?;
    let after_near = split.checked_add(radius)?;
    let after_far = split.checked_add(twice)?;
    Some(match side {
        Side::Negative => [
            (before_far, before_near),
            (before_near, split),
            (split, after_near),
            (after_near, after_far),
        ],
        Side::Positive => [
            (after_near, after_far),
            (split, after_near),
            (before_near, split),
            (before_far, before_near),
        ],
    })
}

fn log_band<F>(start: u32, end: u32, sample: F) -> Option<BandStatistics>
where
    F: Fn(u32) -> Option<Rgb>,
{
    if end <= start {
        return None;
    }
    let expected = end - start;
    let mut sum = [0.0; 3];
    let mut count = 0_u32;
    for axis in start..end {
        if let Some(value) = sample(axis).and_then(log_rgb) {
            sum = add(sum, value);
            count += 1;
        }
    }
    if count < expected.div_ceil(2) {
        return None;
    }
    let mean = scale(sum, 1.0 / f64::from(count));
    let mut dispersion = 0.0;
    for axis in start..end {
        if let Some(value) = sample(axis).and_then(log_rgb) {
            dispersion += norm(sub(value, mean));
        }
    }
    Some(BandStatistics {
        mean,
        dispersion: dispersion / f64::from(count),
        coverage: f64::from(count) / f64::from(expected),
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
        // The outer canonical image is the fixed target. Only this gain is
        // ever applied, and it is applied solely to the inner reference.
        gain: sub(outer_at_boundary, inner_at_boundary),
        weight,
    })
}

fn matched_gain_profiles(
    first: &[Option<GainObservation>],
    second: &[Option<GainObservation>],
) -> (Vec<Rgb>, Vec<Rgb>) {
    let samples: Vec<(Rgb, f64)> = first
        .iter()
        .chain(second)
        .filter_map(|item| item.map(|observation| (observation.gain, observation.weight)))
        .collect();
    let estimate = robust_rgb(&samples);
    let center = estimate.map_or([0.0; 3], |value| value.center);
    let dispersion = estimate.map_or(0.01, |value| value.dispersion);
    (
        stabilize_gain_profile(first, center, dispersion),
        stabilize_gain_profile(second, center, dispersion),
    )
}

fn stabilize_gain_profile(
    observations: &[Option<GainObservation>],
    center: Rgb,
    dispersion: f64,
) -> Vec<Rgb> {
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
    let radius = GAIN_PROFILE_SMOOTH_RADIUS.min(observations.len().saturating_sub(1) / 2);
    let limit = MAX_MATCH_GAIN_STOPS * std::f64::consts::LN_2;
    smooth_profile(&raw, radius)
        .into_iter()
        .map(|gain| gain.map(|channel| channel.clamp(-limit, limit)))
        .collect()
}

fn center_gain_profile(
    first_gain: &[Rgb],
    second_gain: &[Rgb],
    first_distance: &[f64],
    second_distance: &[f64],
) -> Vec<Rgb> {
    first_gain
        .iter()
        .zip(second_gain)
        .zip(first_distance.iter().zip(second_distance))
        .map(|((first, second), (first_extent, second_extent))| {
            let total = first_extent + second_extent;
            if total <= 0.0 {
                scale(add(*first, *second), 0.5)
            } else {
                add(
                    scale(*first, second_extent / total),
                    scale(*second, first_extent / total),
                )
            }
        })
        .collect()
}

impl VerticalPlan {
    fn sample(&self, reference: &PngStage, x: u32, y: u32) -> Option<(f64, Rgb)> {
        if y >= self.geometry.height
            || x < self.geometry.origin_x
            || x >= self.geometry.origin_x + self.geometry.width
        {
            return None;
        }
        let index = y as usize;
        let (distance, extent, edge_gain) = if x < self.seam {
            (
                f64::from(self.seam - 1 - x),
                self.left_distance[index],
                self.left_gain[index],
            )
        } else {
            (
                f64::from(x - self.seam),
                self.right_distance[index],
                self.right_gain[index],
            )
        };
        let alpha = raised_cosine(distance, extent);
        if alpha <= 0.0 {
            return None;
        }
        let local_x = x - self.geometry.origin_x;
        let local_y = y - self.geometry.origin_y;
        let rgb = reference.linear_rgb_with_transfer(local_x, local_y, TRANSFER)?;
        let outward = smooth_step((distance / extent).clamp(0.0, 1.0));
        let gain = interpolate_rgb(self.center_gain[index], edge_gain, outward);
        Some((alpha, apply_log_gain(rgb, gain)))
    }
}

impl HorizontalPlan {
    fn sample(&self, reference: &PngStage, x: u32, y: u32) -> Option<(f64, Rgb)> {
        if x >= self.geometry.width
            || y < self.geometry.origin_y
            || y >= self.geometry.origin_y + self.geometry.height
        {
            return None;
        }
        let index = x as usize;
        let (distance, extent, edge_gain) = if y < self.seam {
            (
                f64::from(self.seam - 1 - y),
                self.top_distance[index],
                self.top_gain[index],
            )
        } else {
            (
                f64::from(y - self.seam),
                self.bottom_distance[index],
                self.bottom_gain[index],
            )
        };
        let alpha = raised_cosine(distance, extent);
        if alpha <= 0.0 {
            return None;
        }
        let local_x = x - self.geometry.origin_x;
        let local_y = y - self.geometry.origin_y;
        let rgb = reference.linear_rgb_with_transfer(local_x, local_y, TRANSFER)?;
        let outward = smooth_step((distance / extent).clamp(0.0, 1.0));
        let gain = interpolate_rgb(self.center_gain[index], edge_gain, outward);
        Some((alpha, apply_log_gain(rgb, gain)))
    }
}

fn raised_cosine(distance: f64, extent: f64) -> f64 {
    if extent <= 0.0 || distance >= extent {
        0.0
    } else {
        0.5 * (1.0 + (PI * (distance / extent).clamp(0.0, 1.0)).cos())
    }
}

fn smooth_step(value: f64) -> f64 {
    value * value * (3.0 - 2.0 * value)
}

fn interpolate_rgb(start: Rgb, end: Rgb, amount: f64) -> Rgb {
    std::array::from_fn(|channel| start[channel] + (end[channel] - start[channel]) * amount)
}

fn combine_references(
    base: Rgb,
    vertical: Option<(f64, Rgb)>,
    horizontal: Option<(f64, Rgb)>,
) -> Rgb {
    let (vertical_alpha, vertical_rgb) = vertical.unwrap_or((0.0, base));
    let (horizontal_alpha, horizontal_rgb) = horizontal.unwrap_or((0.0, base));
    let evidence = vertical_alpha + horizontal_alpha;
    if evidence <= 0.0 {
        return base;
    }
    let union = 1.0 - (1.0 - vertical_alpha) * (1.0 - horizontal_alpha);
    // The portrait reference is continuous across X but retains its own Y
    // join; the landscape reference has the opposite property. Suppress each
    // reference as the other axis becomes authoritative. A small floor keeps
    // the exact X/Y intersection symmetric instead of becoming undefined.
    const INTERSECTION_FLOOR: f64 = 0.05;
    let vertical_evidence = vertical_alpha * ((1.0 - horizontal_alpha) + INTERSECTION_FLOOR);
    let horizontal_evidence = horizontal_alpha * ((1.0 - vertical_alpha) + INTERSECTION_FLOOR);
    let directional_evidence = vertical_evidence + horizontal_evidence;
    let vertical_weight = union * vertical_evidence / directional_evidence;
    let horizontal_weight = union * horizontal_evidence / directional_evidence;
    let base_weight = 1.0 - union;
    std::array::from_fn(|channel| {
        if base[channel] >= 1.0 {
            // Structural replacement must not reproduce the old global-field
            // failure mode where clipped source white became gray.
            1.0
        } else {
            base[channel]
                .mul_add(
                    base_weight,
                    vertical_rgb[channel]
                        .mul_add(vertical_weight, horizontal_rgb[channel] * horizontal_weight),
                )
                .clamp(0.0, 1.0)
        }
    })
}

fn range_report(distances: &[f64]) -> StitchRangeReport {
    let mut values = distances.to_vec();
    let median_distance = median(&mut values).round() as u32;
    StitchRangeReport {
        minimum_distance: distances
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min)
            .round() as u32,
        median_distance,
        maximum_distance: distances.iter().copied().fold(0.0_f64, f64::max).round() as u32,
        scanlines: distances.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_is_full_at_the_original_seam_and_zero_at_the_stitch() {
        assert_eq!(raised_cosine(0.0, 100.0), 1.0);
        assert!((raised_cosine(50.0, 100.0) - 0.5).abs() < 1.0e-12);
        assert_eq!(raised_cosine(100.0, 100.0), 0.0);
        assert_eq!(raised_cosine(120.0, 100.0), 0.0);
    }

    #[test]
    fn one_or_both_crosses_form_a_partition_of_unity() {
        let base = [0.1, 0.2, 0.3];
        let vertical = [0.5, 0.6, 0.7];
        let horizontal = [0.9, 0.8, 0.7];
        assert_eq!(combine_references(base, None, None), base);
        assert_eq!(
            combine_references(base, Some((1.0, vertical)), None),
            vertical
        );
        let center = combine_references(base, Some((1.0, vertical)), Some((1.0, horizontal)));
        assert!((center[0] - 0.7).abs() < 1.0e-12);
        assert!((center[1] - 0.7).abs() < 1.0e-12);
        assert!((center[2] - 0.7).abs() < 1.0e-12);

        let vertical_seam =
            combine_references([0.0; 3], Some((1.0, [1.0; 3])), Some((0.5, [0.0; 3])));
        assert!(vertical_seam[0] > 0.95);
    }

    #[test]
    fn exact_base_white_survives_structural_replacement() {
        assert_eq!(
            combine_references([1.0, 0.5, 1.0], Some((1.0, [0.2, 0.2, 0.2])), None,),
            [1.0, 0.2, 1.0]
        );
    }

    #[test]
    fn cross_geometry_is_centered_without_resampling() {
        let landscape = landscape_geometry(8192, 8192, 4096, (8192, 4096)).unwrap();
        let portrait = portrait_geometry(8192, 8192, 4096, (4096, 8192)).unwrap();
        assert_eq!((landscape.origin_x, landscape.origin_y), (0, 2048));
        assert_eq!((portrait.origin_x, portrait.origin_y), (2048, 0));
        assert!(landscape_geometry(8192, 8192, 4096, (4096, 4096)).is_err());
        assert!(portrait_geometry(8192, 8192, 4096, (4096, 4096)).is_err());
    }

    #[test]
    fn stitch_search_refines_to_the_exact_lowest_difference_position() {
        let selected = best_distance(512, 2015, 2, |distance| {
            Some(f64::from(distance.abs_diff(1337)))
        });
        assert_eq!(selected, 1337);
    }

    #[test]
    fn standard_cross_restores_the_unrestricted_structural_search() {
        let (minimum, maximum, step) = match_search_bounds(2048).unwrap();
        assert_eq!((minimum, maximum, step), (512, 2015, 2));
        assert!(maximum > 7 * 2048 / 8);
    }

    #[test]
    fn boundary_bands_keep_the_outer_base_and_inner_reference_distinct() {
        assert_eq!(
            boundary_ranges(100, Side::Negative, 8).unwrap(),
            [(84, 92), (92, 100), (100, 108), (108, 116)]
        );
        assert_eq!(
            boundary_ranges(100, Side::Positive, 8).unwrap(),
            [(108, 116), (100, 108), (92, 100), (84, 92)]
        );
    }

    #[test]
    fn boundary_gain_maps_only_the_inner_reference_to_the_outer_base() {
        let stats = |value: f64| BandStatistics {
            mean: [value.ln(); 3],
            dispersion: 0.0,
            coverage: 1.0,
        };
        let observation =
            boundary_gain_observation(stats(0.5), stats(0.5), stats(0.25), stats(0.25)).unwrap();
        let corrected = apply_log_gain([0.25; 3], observation.gain);
        for channel in corrected {
            assert!((channel - 0.5).abs() < 1.0e-12);
        }
    }

    #[test]
    fn unequal_structural_extents_share_one_continuous_center_gain() {
        let center = center_gain_profile(&[[1.0; 3]], &[[3.0; 3]], &[100.0], &[300.0]);
        assert_eq!(center, vec![[1.5; 3]]);
    }
}
