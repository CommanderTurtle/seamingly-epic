use std::f64::consts::{LN_2, PI};

use anyhow::{Context, Result, ensure};
use rayon::prelude::*;

use crate::{
    color::{Rgb, add, log_gain_to_stops, log_rgb, norm, scale, sub},
    config::CorrectionConfig,
    layout::Layout,
    report::{
        BoundaryReport, CorrectionReport, FieldReport, ImageReport, Orientation, TileGainReport,
    },
    robust::{RobustEstimate, robust_rgb, smooth_profile},
    solve::{GainConstraint, limit_gains, solve_tile_gains},
};

/// Read-only linear-light pixels consumed by the correction model.
pub(crate) trait PixelSource: Sync {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn linear_rgb(&self, x: u32, y: u32) -> Option<Rgb>;
}

#[derive(Clone, Copy, Debug)]
struct ProfileSample {
    position: u32,
    jump: Rgb,
    weight: f64,
}

#[derive(Clone, Debug)]
struct BoundaryAnalysis {
    orientation: Orientation,
    nominal_coordinate: u32,
    coordinate: u32,
    segment_index: usize,
    segment_start: u32,
    segment_end: u32,
    tile_a: usize,
    tile_b: usize,
    estimate: Option<RobustEstimate>,
    confidence: f64,
    samples: Vec<ProfileSample>,
}

impl BoundaryAnalysis {
    fn accepted(&self, config: &CorrectionConfig) -> bool {
        self.estimate.is_some() && self.confidence > 0.0 && self.confidence >= config.min_confidence
    }

    fn to_report(&self, config: &CorrectionConfig) -> BoundaryReport {
        let estimate = self.estimate.unwrap_or(RobustEstimate {
            center: [0.0; 3],
            dispersion: 0.0,
            effective_weight: 0.0,
        });
        BoundaryReport {
            orientation: self.orientation,
            nominal_coordinate: self.nominal_coordinate,
            coordinate: self.coordinate,
            segment_index: self.segment_index,
            segment_start: self.segment_start,
            segment_end: self.segment_end,
            tile_a: self.tile_a,
            tile_b: self.tile_b,
            log_jump_rgb: estimate.center,
            jump_stops_rgb: log_gain_to_stops(estimate.center),
            dispersion: estimate.dispersion,
            confidence: self.confidence,
            valid_samples: self.samples.len(),
            accepted: self.accepted(config),
        }
    }
}

#[derive(Clone, Debug)]
struct LocalField {
    coordinate: u32,
    segment_start: u32,
    segment_end: u32,
    target_difference: Vec<Rgb>,
    side_a: Vec<Rgb>,
    side_b: Vec<Rgb>,
}

impl LocalField {
    fn sample_index(&self, position: u32) -> Option<usize> {
        if self.target_difference.is_empty() {
            return None;
        }
        Some(
            position
                .saturating_sub(self.segment_start)
                .min(self.segment_end - self.segment_start - 1) as usize,
        )
    }

    fn sample(profile: &[Rgb], index: Option<usize>) -> Rgb {
        let Some(index) = index else {
            return [0.0; 3];
        };
        profile[index]
    }

    fn side_a_at(&self, position: u32) -> Rgb {
        Self::sample(&self.side_a, self.sample_index(position))
    }

    fn side_b_at(&self, position: u32) -> Rgb {
        Self::sample(&self.side_b, self.sample_index(position))
    }
}

/// Fully analyzed correction field. PNG and float32 transports share it.
pub(crate) struct CorrectionModel {
    pub report: CorrectionReport,
    vertical: Vec<Option<LocalField>>,
    horizontal: Vec<Option<LocalField>>,
    max_log_gain: f64,
}

impl CorrectionModel {
    #[must_use]
    pub fn log_gain_at(&self, x: u32, y: u32) -> Rgb {
        wave_gain_at(&self.report.layout, &self.vertical, &self.horizontal, x, y)
            .map(|value| value.clamp(-self.max_log_gain, self.max_log_gain))
    }
}

fn raised_cosine(unit_distance: f64) -> f64 {
    if unit_distance >= 1.0 {
        0.0
    } else {
        0.5 * (1.0 + (PI * unit_distance.max(0.0)).cos())
    }
}

/// Evaluate a seam-normal wave which is exactly one at the two samples touching
/// the join and exactly zero at the center of the neighboring tile. The other
/// half of that tile is untouched. This makes the source tile interior the
/// gauge: correction cannot turn into an image-wide exposure shift.
fn midpoint_wave(position: u32, seam: u32, outer_edge: u32, before_seam: bool) -> f64 {
    let (boundary, anchor) = if before_seam {
        let width = seam.saturating_sub(outer_edge);
        (seam - 1, outer_edge + width / 2)
    } else {
        let width = outer_edge.saturating_sub(seam);
        (seam, seam + width / 2)
    };

    if position == boundary {
        return 1.0;
    }
    if boundary == anchor {
        return 0.0;
    }

    let unit_distance = if before_seam {
        if position <= anchor || position > boundary {
            return 0.0;
        }
        f64::from(boundary - position) / f64::from(boundary - anchor)
    } else {
        if position >= anchor || position < boundary {
            return 0.0;
        }
        f64::from(position - boundary) / f64::from(anchor - boundary)
    };
    raised_cosine(unit_distance)
}

fn wave_gain_at(
    layout: &Layout,
    vertical: &[Option<LocalField>],
    horizontal: &[Option<LocalField>],
    x: u32,
    y: u32,
) -> Rgb {
    let column = layout.x_seams.partition_point(|seam| x >= *seam);
    let row = layout.y_seams.partition_point(|seam| y >= *seam);
    let mut gain = [0.0; 3];

    // The seam on this tile's left contributes its B/right-side endpoint.
    if column > 0 {
        let seam_index = column - 1;
        let field_index = seam_index * layout.rows() + row;
        if let Some(field) = &vertical[field_index] {
            let outer_edge = layout.x_seams.get(column).copied().unwrap_or(layout.width);
            let wave = midpoint_wave(x, field.coordinate, outer_edge, false);
            gain = add(gain, scale(field.side_b_at(y), wave));
        }
    }

    // The seam on this tile's right contributes its A/left-side endpoint.
    if column < layout.x_seams.len() {
        let field_index = column * layout.rows() + row;
        if let Some(field) = &vertical[field_index] {
            let outer_edge = column
                .checked_sub(1)
                .and_then(|index| layout.x_seams.get(index))
                .copied()
                .unwrap_or(0);
            let wave = midpoint_wave(x, field.coordinate, outer_edge, true);
            gain = add(gain, scale(field.side_a_at(y), wave));
        }
    }

    // The seam above this tile contributes its B/bottom-side endpoint.
    if row > 0 {
        let seam_index = row - 1;
        let field_index = seam_index * layout.columns() + column;
        if let Some(field) = &horizontal[field_index] {
            let outer_edge = layout.y_seams.get(row).copied().unwrap_or(layout.height);
            let wave = midpoint_wave(y, field.coordinate, outer_edge, false);
            gain = add(gain, scale(field.side_b_at(x), wave));
        }
    }

    // The seam below this tile contributes its A/top-side endpoint.
    if row < layout.y_seams.len() {
        let field_index = row * layout.columns() + column;
        if let Some(field) = &horizontal[field_index] {
            let outer_edge = row
                .checked_sub(1)
                .and_then(|index| layout.y_seams.get(index))
                .copied()
                .unwrap_or(0);
            let wave = midpoint_wave(y, field.coordinate, outer_edge, true);
            gain = add(gain, scale(field.side_a_at(x), wave));
        }
    }

    gain
}

#[derive(Clone, Copy, Debug, Default)]
struct WaveRefinement {
    passes: u32,
    initial_max_residual: f64,
    final_max_residual: f64,
}

fn residual_magnitude(value: Rgb) -> f64 {
    value.into_iter().map(f64::abs).fold(0.0, f64::max)
}

fn max_boundary_residual(
    layout: &Layout,
    vertical: &[Option<LocalField>],
    horizontal: &[Option<LocalField>],
) -> f64 {
    let mut maximum = 0.0_f64;
    for field in vertical.iter().flatten() {
        for (offset, target) in field.target_difference.iter().copied().enumerate() {
            let y = field.segment_start + offset as u32;
            let left = wave_gain_at(layout, vertical, horizontal, field.coordinate - 1, y);
            let right = wave_gain_at(layout, vertical, horizontal, field.coordinate, y);
            maximum = maximum.max(residual_magnitude(sub(target, sub(right, left))));
        }
    }
    for field in horizontal.iter().flatten() {
        for (offset, target) in field.target_difference.iter().copied().enumerate() {
            let x = field.segment_start + offset as u32;
            let top = wave_gain_at(layout, vertical, horizontal, x, field.coordinate - 1);
            let bottom = wave_gain_at(layout, vertical, horizontal, x, field.coordinate);
            maximum = maximum.max(residual_magnitude(sub(target, sub(bottom, top))));
        }
    }
    maximum
}

fn clamp_profile(value: Rgb, limit: f64) -> Rgb {
    value.map(|channel| channel.clamp(-limit, limit))
}

fn refine_vertical(
    layout: &Layout,
    vertical: &mut [Option<LocalField>],
    horizontal: &[Option<LocalField>],
    relaxation: f64,
    limit: f64,
) {
    for field_index in 0..vertical.len() {
        let length = vertical[field_index]
            .as_ref()
            .map_or(0, |field| field.target_difference.len());
        for offset in 0..length {
            let (coordinate, y, target) = {
                let field = vertical[field_index]
                    .as_ref()
                    .expect("accepted vertical field exists");
                (
                    field.coordinate,
                    field.segment_start + offset as u32,
                    field.target_difference[offset],
                )
            };
            let left = wave_gain_at(layout, vertical, horizontal, coordinate - 1, y);
            let right = wave_gain_at(layout, vertical, horizontal, coordinate, y);
            let residual = sub(target, sub(right, left));
            let half_step = scale(residual, 0.5 * relaxation);
            let field = vertical[field_index]
                .as_mut()
                .expect("accepted vertical field exists");
            field.side_a[offset] = clamp_profile(sub(field.side_a[offset], half_step), limit);
            field.side_b[offset] = clamp_profile(add(field.side_b[offset], half_step), limit);
        }
    }
}

fn refine_horizontal(
    layout: &Layout,
    vertical: &[Option<LocalField>],
    horizontal: &mut [Option<LocalField>],
    relaxation: f64,
    limit: f64,
) {
    for field_index in 0..horizontal.len() {
        let length = horizontal[field_index]
            .as_ref()
            .map_or(0, |field| field.target_difference.len());
        for offset in 0..length {
            let (x, coordinate, target) = {
                let field = horizontal[field_index]
                    .as_ref()
                    .expect("accepted horizontal field exists");
                (
                    field.segment_start + offset as u32,
                    field.coordinate,
                    field.target_difference[offset],
                )
            };
            let top = wave_gain_at(layout, vertical, horizontal, x, coordinate - 1);
            let bottom = wave_gain_at(layout, vertical, horizontal, x, coordinate);
            let residual = sub(target, sub(bottom, top));
            let half_step = scale(residual, 0.5 * relaxation);
            let field = horizontal[field_index]
                .as_mut()
                .expect("accepted horizontal field exists");
            field.side_a[offset] = clamp_profile(sub(field.side_a[offset], half_step), limit);
            field.side_b[offset] = clamp_profile(add(field.side_b[offset], half_step), limit);
        }
    }
}

/// Alternating boundary projections remove the small cross-axis residuals that
/// can occur where two independently varying seam profiles intersect. Each
/// speculative pass is accepted only when it improves the measured maximum;
/// otherwise it is rolled back and retried with a smaller relaxation factor.
fn refine_wave_fields(
    layout: &Layout,
    vertical: &mut [Option<LocalField>],
    horizontal: &mut [Option<LocalField>],
    max_log_gain: f64,
) -> WaveRefinement {
    const TARGET: f64 = 1.0e-8;
    const MAX_ATTEMPTS: usize = 32;

    let initial = max_boundary_residual(layout, vertical, horizontal);
    if initial <= TARGET {
        return WaveRefinement {
            initial_max_residual: initial,
            final_max_residual: initial,
            ..WaveRefinement::default()
        };
    }

    let mut previous = initial;
    let mut relaxation = 0.75;
    let mut passes = 0_u32;
    for _ in 0..MAX_ATTEMPTS {
        let saved_vertical = vertical.to_vec();
        let saved_horizontal = horizontal.to_vec();
        refine_vertical(layout, vertical, horizontal, relaxation, max_log_gain);
        refine_horizontal(layout, vertical, horizontal, relaxation, max_log_gain);
        let next = max_boundary_residual(layout, vertical, horizontal);
        if !next.is_finite() || next > previous * (1.0 + 1.0e-10) {
            vertical.clone_from_slice(&saved_vertical);
            horizontal.clone_from_slice(&saved_horizontal);
            relaxation *= 0.5;
            if relaxation < 0.031_25 {
                break;
            }
            continue;
        }

        passes += 1;
        let improvement = previous - next;
        previous = next;
        if next <= TARGET || improvement <= TARGET * 0.01 {
            break;
        }
        relaxation = (relaxation * 1.08_f64).min(0.9_f64);
    }

    WaveRefinement {
        passes,
        initial_max_residual: initial,
        final_max_residual: previous,
    }
}

pub(crate) fn build_model<S: PixelSource>(
    source: &S,
    config: &CorrectionConfig,
    image: ImageReport,
) -> Result<CorrectionModel> {
    validate_config(config)?;
    ensure!(
        source.width() == image.width && source.height() == image.height,
        "pixel source and image report dimensions disagree"
    );
    let layout = Layout::resolve(source.width(), source.height(), &config.seams)?;

    let analyses = with_threads(config.threads, || analyze_all(source, &layout, config))?;
    let constraints: Vec<GainConstraint> = analyses
        .iter()
        .filter(|analysis| analysis.accepted(config))
        .filter_map(|analysis| {
            analysis.estimate.map(|estimate| GainConstraint {
                left_or_top: analysis.tile_a,
                right_or_bottom: analysis.tile_b,
                jump: estimate.center,
                weight: analysis.confidence.powi(2) * estimate.effective_weight.max(0.05),
            })
        })
        .collect();

    let graph_connected = constraint_graph_connected(layout.tile_count(), &constraints);
    let mut gains = if graph_connected {
        solve_tile_gains(layout.tile_count(), &constraints)?
    } else {
        // A graph gauge cannot cross a rejected boundary without inventing a
        // relationship. Disconnected analyses therefore use neutral gauges;
        // accepted per-position evidence still creates symmetric local waves.
        vec![[0.0; 3]; layout.tile_count()]
    };
    limit_gains(&mut gains, config.strength, config.max_gain_stops);

    let mut vertical = vec![None; layout.x_seams.len() * layout.rows()];
    let mut horizontal = vec![None; layout.y_seams.len() * layout.columns()];
    for analysis in &analyses {
        if !analysis.accepted(config) {
            continue;
        }
        let field = make_local_field(
            analysis,
            gains[analysis.tile_a],
            gains[analysis.tile_b],
            config,
        );
        match analysis.orientation {
            Orientation::Vertical => {
                let seam_index = layout
                    .x_seams
                    .iter()
                    .position(|value| *value == analysis.nominal_coordinate)
                    .context("vertical seam disappeared from layout")?;
                vertical[seam_index * layout.rows() + analysis.segment_index] = Some(field);
            }
            Orientation::Horizontal => {
                let seam_index = layout
                    .y_seams
                    .iter()
                    .position(|value| *value == analysis.nominal_coordinate)
                    .context("horizontal seam disappeared from layout")?;
                horizontal[seam_index * layout.columns() + analysis.segment_index] = Some(field);
            }
        }
    }

    let accepted = analyses
        .iter()
        .filter(|analysis| analysis.accepted(config))
        .count();
    let seam_impulses = vertical
        .iter()
        .chain(horizontal.iter())
        .flatten()
        .map(|field| field.target_difference.len() as u64)
        .sum::<u64>();
    let max_log_gain = config.max_gain_stops.max(0.0) * LN_2;
    let refinement = if accepted > 0 {
        refine_wave_fields(&layout, &mut vertical, &mut horizontal, max_log_gain)
    } else {
        WaveRefinement::default()
    };

    let mut warnings = Vec::new();
    if accepted == 0 {
        warnings.push(
            "No boundary passed the confidence threshold; RGB samples were left unchanged."
                .to_owned(),
        );
    } else if accepted < analyses.len() {
        warnings.push(format!(
            "Accepted {accepted} of {} boundary segments; rejected segments were left unchanged.",
            analyses.len()
        ));
    }
    if accepted > 0 && !graph_connected {
        warnings.push(
            "Accepted constraints do not connect every tile; graph endpoint gauges were disabled while accepted midpoint-anchored seam waves remained active."
                .to_owned(),
        );
    }
    let tile_gains = gains
        .iter()
        .enumerate()
        .map(|(tile, gain)| TileGainReport {
            tile,
            row: tile / layout.columns(),
            column: tile % layout.columns(),
            log_gain_rgb: *gain,
            gain_stops_rgb: log_gain_to_stops(*gain),
        })
        .collect();
    let boundaries = analyses
        .iter()
        .map(|analysis| analysis.to_report(config))
        .collect();
    let output_pixels = u64::from(image.width) * u64::from(image.height);
    let tile_count = layout.tile_count() as u64;
    let report = CorrectionReport {
        version: 3,
        image,
        layout,
        config: config.clone(),
        boundaries,
        tile_gains,
        field: FieldReport {
            strategy: if accepted > 0 {
                "tile_laplacian_midpoint_anchored_raised_cosine".to_owned()
            } else {
                "disabled".to_owned()
            },
            precision: "f64_log_linear_rgb".to_owned(),
            seam_impulses,
            conceptual_tile_relationships: tile_count.saturating_mul(tile_count),
            output_pixels,
            stored_field_bytes: seam_impulses
                .saturating_mul(3)
                .saturating_mul(size_of::<Rgb>() as u64),
            headroom_shift_stops: 0.0,
            neutral_interior_anchors: tile_count,
            refinement_passes: refinement.passes,
            initial_max_residual_stops: refinement.initial_max_residual / LN_2,
            final_max_residual_stops: refinement.final_max_residual / LN_2,
        },
        warnings,
        applied: accepted > 0,
    };

    Ok(CorrectionModel {
        report,
        vertical,
        horizontal,
        max_log_gain,
    })
}

fn constraint_graph_connected(tile_count: usize, constraints: &[GainConstraint]) -> bool {
    if tile_count <= 1 {
        return true;
    }
    let mut adjacency = vec![Vec::new(); tile_count];
    for constraint in constraints.iter().filter(|item| item.weight > 0.0) {
        adjacency[constraint.left_or_top].push(constraint.right_or_bottom);
        adjacency[constraint.right_or_bottom].push(constraint.left_or_top);
    }
    let mut seen = vec![false; tile_count];
    let mut stack = vec![0];
    seen[0] = true;
    while let Some(tile) = stack.pop() {
        for neighbor in &adjacency[tile] {
            if !seen[*neighbor] {
                seen[*neighbor] = true;
                stack.push(*neighbor);
            }
        }
    }
    seen.into_iter().all(|value| value)
}

fn validate_config(config: &CorrectionConfig) -> Result<()> {
    ensure!(
        config.scan_radius > 0,
        "scan radius must be at least one pixel"
    );
    ensure!(
        config.sample_stride > 0,
        "sample stride must be at least one pixel"
    );
    ensure!(config.strength.is_finite(), "strength must be finite");
    ensure!(
        config.local_strength.is_finite(),
        "local strength must be finite"
    );
    ensure!(
        config.max_gain_stops.is_finite() && config.max_gain_stops >= 0.0,
        "maximum gain must be finite and non-negative"
    );
    ensure!(
        config.min_confidence.is_finite() && (0.0..=1.0).contains(&config.min_confidence),
        "minimum confidence must be between zero and one"
    );
    Ok(())
}

fn analyze_all<S: PixelSource>(
    source: &S,
    layout: &Layout,
    config: &CorrectionConfig,
) -> Result<Vec<BoundaryAnalysis>> {
    #[derive(Clone, Copy)]
    struct Task {
        orientation: Orientation,
        coordinate: u32,
        segment: usize,
        start: u32,
        end: u32,
        tile_a: usize,
        tile_b: usize,
    }

    let mut tasks = Vec::new();
    let x_edges = layout.x_edges();
    let y_edges = layout.y_edges();
    for (seam_index, coordinate) in layout.x_seams.iter().copied().enumerate() {
        for row in 0..layout.rows() {
            tasks.push(Task {
                orientation: Orientation::Vertical,
                coordinate,
                segment: row,
                start: y_edges[row],
                end: y_edges[row + 1],
                tile_a: layout.tile_index(seam_index, row),
                tile_b: layout.tile_index(seam_index + 1, row),
            });
        }
    }
    for (seam_index, coordinate) in layout.y_seams.iter().copied().enumerate() {
        for column in 0..layout.columns() {
            tasks.push(Task {
                orientation: Orientation::Horizontal,
                coordinate,
                segment: column,
                start: x_edges[column],
                end: x_edges[column + 1],
                tile_a: layout.tile_index(column, seam_index),
                tile_b: layout.tile_index(column, seam_index + 1),
            });
        }
    }

    tasks
        .par_iter()
        .map(|task| {
            Ok(analyze_boundary(
                source,
                task.orientation,
                task.coordinate,
                task.segment,
                task.start,
                task.end,
                task.tile_a,
                task.tile_b,
                config,
            ))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn analyze_boundary<S: PixelSource>(
    source: &S,
    orientation: Orientation,
    nominal: u32,
    segment_index: usize,
    segment_start: u32,
    segment_end: u32,
    tile_a: usize,
    tile_b: usize,
    config: &CorrectionConfig,
) -> BoundaryAnalysis {
    let axis_limit = match orientation {
        Orientation::Vertical => source.width(),
        Orientation::Horizontal => source.height(),
    };
    let search_start = nominal.saturating_sub(config.refine_radius).max(2);
    let search_end = nominal
        .saturating_add(config.refine_radius)
        .min(axis_limit.saturating_sub(2));

    let mut candidates = Vec::new();
    for coordinate in search_start..=search_end {
        let samples = collect_samples(
            source,
            orientation,
            coordinate,
            segment_start,
            segment_end,
            config,
        );
        let weighted: Vec<(Rgb, f64)> = samples
            .iter()
            .map(|sample| (sample.jump, sample.weight))
            .collect();
        let estimate = robust_rgb(&weighted);
        let expected = expected_sample_count(segment_start, segment_end, config.sample_stride);
        let confidence = boundary_confidence(estimate, samples.len(), expected);
        let score = estimate.map_or(0.0, |value| {
            confidence * norm(value.center) / (value.dispersion + 0.002)
        });
        candidates.push((coordinate, samples, estimate, confidence, score));
    }

    // Prefer the nominal coordinate unless a neighboring scan is materially
    // more coherent. This avoids drifting onto an unrelated high-contrast line.
    let nominal_score = candidates
        .iter()
        .find(|candidate| candidate.0 == nominal)
        .map_or(0.0, |candidate| candidate.4);
    let best_index = candidates
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.4.total_cmp(&right.4))
        .map_or(0, |(index, _)| index);
    let nominal_index = candidates
        .iter()
        .position(|candidate| candidate.0 == nominal)
        .unwrap_or(best_index);
    let selected = if candidates[best_index].4 > nominal_score * 1.20 + 1.0e-9 {
        best_index
    } else {
        nominal_index
    };
    let (coordinate, samples, estimate, confidence, _) = candidates.swap_remove(selected);

    BoundaryAnalysis {
        orientation,
        nominal_coordinate: nominal,
        coordinate,
        segment_index,
        segment_start,
        segment_end,
        tile_a,
        tile_b,
        estimate,
        confidence,
        samples,
    }
}

fn expected_sample_count(start: u32, end: u32, stride: u32) -> usize {
    usize::try_from(end.saturating_sub(start).div_ceil(stride)).unwrap_or(usize::MAX)
}

fn boundary_confidence(
    estimate: Option<RobustEstimate>,
    valid_samples: usize,
    expected_samples: usize,
) -> f64 {
    let Some(estimate) = estimate else {
        return 0.0;
    };
    let coverage = if expected_samples == 0 {
        0.0
    } else {
        valid_samples as f64 / expected_samples as f64
    };
    let texture_weight = estimate.effective_weight / valid_samples.max(1) as f64;
    let signal = norm(estimate.center);
    let coherence = signal / (signal + 2.0 * estimate.dispersion + 0.002);
    (coverage.sqrt() * texture_weight.clamp(0.0, 1.0).sqrt() * coherence).clamp(0.0, 1.0)
}

fn collect_samples<S: PixelSource>(
    source: &S,
    orientation: Orientation,
    coordinate: u32,
    segment_start: u32,
    segment_end: u32,
    config: &CorrectionConfig,
) -> Vec<ProfileSample> {
    let axis_limit = match orientation {
        Orientation::Vertical => source.width(),
        Orientation::Horizontal => source.height(),
    };
    let available = coordinate.min(axis_limit.saturating_sub(coordinate));
    let radius = config.scan_radius.min(available / 2);
    if radius == 0 || segment_end <= segment_start {
        return Vec::new();
    }

    (segment_start..segment_end)
        .step_by(config.sample_stride as usize)
        .filter_map(|position| {
            sample_boundary(source, orientation, coordinate, position, radius).map(
                |(jump, weight)| ProfileSample {
                    position,
                    jump,
                    weight,
                },
            )
        })
        .collect()
}

fn sample_boundary<S: PixelSource>(
    source: &S,
    orientation: Orientation,
    coordinate: u32,
    position: u32,
    radius: u32,
) -> Option<(Rgb, f64)> {
    let band = |start: u32, end: u32| -> Option<(Rgb, f64, f64)> {
        let values: Vec<Rgb> = (start..end)
            .filter_map(|axis| {
                let (x, y) = match orientation {
                    Orientation::Vertical => (axis, position),
                    Orientation::Horizontal => (position, axis),
                };
                source.linear_rgb(x, y).and_then(log_rgb)
            })
            .collect();
        if values.len() < (radius as usize).div_ceil(2) {
            return None;
        }
        let mean = scale(
            values.iter().copied().fold([0.0; 3], add),
            1.0 / values.len() as f64,
        );
        let dispersion = values
            .iter()
            .map(|value| norm(sub(*value, mean)))
            .sum::<f64>()
            / values.len() as f64;
        let coverage = values.len() as f64 / f64::from(radius);
        Some((mean, dispersion, coverage))
    };

    let left_far = band(coordinate - 2 * radius, coordinate - radius)?;
    let left_near = band(coordinate - radius, coordinate)?;
    let right_near = band(coordinate, coordinate + radius)?;
    let right_far = band(coordinate + radius, coordinate + 2 * radius)?;

    // Extrapolating both sides to the join cancels a natural linear gradient.
    let left_at_boundary = sub(scale(left_near.0, 1.5), scale(left_far.0, 0.5));
    let right_at_boundary = sub(scale(right_near.0, 1.5), scale(right_far.0, 0.5));
    let jump = sub(right_at_boundary, left_at_boundary);
    let texture = left_far.1 + left_near.1 + right_near.1 + right_far.1;
    let coverage = left_far
        .2
        .min(left_near.2)
        .min(right_near.2)
        .min(right_far.2);
    let texture_weight = 1.0 / (1.0 + (texture / 0.12).powi(2));
    Some((jump, coverage * texture_weight))
}

fn make_local_field(
    analysis: &BoundaryAnalysis,
    gain_a: Rgb,
    gain_b: Rgb,
    config: &CorrectionConfig,
) -> LocalField {
    let length = usize::try_from(analysis.segment_end - analysis.segment_start)
        .expect("image dimension fits usize");
    let estimate = analysis.estimate.expect("accepted boundary has estimate");
    let cutoff = (3.0 * estimate.dispersion).max(0.01);
    let mut known = Vec::with_capacity(analysis.samples.len());
    for sample in &analysis.samples {
        let deviation = sub(sample.jump, estimate.center);
        let magnitude = norm(deviation);
        let huber = if magnitude <= cutoff {
            1.0
        } else {
            cutoff / magnitude.max(1.0e-12)
        };
        let stabilized = add(
            estimate.center,
            scale(deviation, huber * sample.weight.sqrt()),
        );
        known.push((
            usize::try_from(sample.position - analysis.segment_start)
                .expect("profile position fits usize"),
            stabilized,
        ));
    }

    let fallback = estimate.center;
    let mut profile = vec![fallback; length];
    if let Some(&(first_position, first_value)) = known.first() {
        profile[..first_position.min(length)].fill(first_value);
        for pair in known.windows(2) {
            let (start, start_value) = pair[0];
            let (end, end_value) = pair[1];
            let span = end.saturating_sub(start).max(1);
            for (offset, output) in profile
                .iter_mut()
                .enumerate()
                .take(end.min(length) + 1)
                .skip(start.min(length))
            {
                let mix = (offset.saturating_sub(start)) as f64 / span as f64;
                *output = add(scale(start_value, 1.0 - mix), scale(end_value, mix));
            }
        }
        if let Some(&(last_position, last_value)) = known.last() {
            profile[last_position.min(length)..].fill(last_value);
        }
    }
    let mut profile = smooth_profile(&profile, config.profile_smooth_radius as usize);
    let residual_limit = config.max_gain_stops.max(0.0) * LN_2 * 2.0;
    for value in &mut profile {
        *value = value.map(|channel| channel.clamp(-residual_limit, residual_limit));
    }

    // The sparse tile Laplacian supplies globally consistent endpoint gauges,
    // but those gauges are never broadcast across a complete tile. For every
    // scanline sample, split the remaining mismatch between the two sides and
    // later decay each endpoint independently to the tile's neutral midpoint.
    let graph_difference = sub(gain_b, gain_a);
    let local_strength = config.local_strength.clamp(0.0, 2.0);
    let endpoint_limit = config.max_gain_stops.max(0.0) * LN_2;
    let mut target_difference = Vec::with_capacity(profile.len());
    let mut side_a = Vec::with_capacity(profile.len());
    let mut side_b = Vec::with_capacity(profile.len());
    for jump in profile {
        let residual = add(jump, graph_difference);
        let half_residual = scale(residual, 0.5 * local_strength);
        let before = clamp_profile(add(gain_a, half_residual), endpoint_limit);
        let after = clamp_profile(sub(gain_b, half_residual), endpoint_limit);
        target_difference.push(add(
            scale(graph_difference, 1.0 - local_strength),
            scale(jump, -local_strength),
        ));
        side_a.push(before);
        side_b.push(after);
    }

    LocalField {
        coordinate: analysis.coordinate,
        segment_start: analysis.segment_start,
        segment_end: analysis.segment_end,
        target_difference,
        side_a,
        side_b,
    }
}

pub(crate) fn with_threads<T, F>(threads: usize, operation: F) -> Result<T>
where
    T: Send,
    F: FnOnce() -> Result<T> + Send,
{
    if threads == 0 {
        operation()
    } else {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .context("could not create the requested worker pool")?
            .install(operation)
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_abs_diff_eq;

    use super::*;
    use crate::config::{GridSpec, SeamSpec, TransferFunction};

    struct Synthetic {
        width: u32,
        height: u32,
        right_gain: f64,
    }

    impl PixelSource for Synthetic {
        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn linear_rgb(&self, x: u32, y: u32) -> Option<Rgb> {
            let gradient = 0.25 + f64::from(x) * 0.001 + f64::from(y) * 0.000_2;
            let gain = if x < self.width / 2 {
                1.0
            } else {
                self.right_gain
            };
            Some([gradient * gain; 3])
        }
    }

    /// Synthetic tile pair whose VAE-like color/exposure offset changes at
    /// every Y position. This exercises the per-position seam profile rather
    /// than only the globally solved tile gain.
    struct VariableSeam {
        width: u32,
        height: u32,
    }

    impl VariableSeam {
        fn jump(&self, y: u32) -> Rgb {
            let phase = 2.0 * PI * f64::from(y) / f64::from(self.height);
            [
                0.08 + 0.035 * phase.sin(),
                -0.03 + 0.02 * phase.cos(),
                0.04 + 0.025 * (phase + 0.7).sin(),
            ]
        }
    }

    impl PixelSource for VariableSeam {
        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn linear_rgb(&self, x: u32, y: u32) -> Option<Rgb> {
            let base = [
                0.28 + 0.000_10 * f64::from(y),
                0.24 + 0.000_08 * f64::from(y),
                0.20 + 0.000_06 * f64::from(y),
            ];
            Some(if x < self.width / 2 {
                base
            } else {
                crate::color::apply_log_gain(base, self.jump(y))
            })
        }
    }

    struct MultiTileGrid {
        width: u32,
        height: u32,
        columns: u32,
        rows: u32,
    }

    impl MultiTileGrid {
        fn tile_gain(&self, x: u32, y: u32) -> Rgb {
            let column = (x * self.columns / self.width).min(self.columns - 1);
            let row = (y * self.rows / self.height).min(self.rows - 1);
            [
                0.03 * f64::from(column) - 0.02 * f64::from(row),
                -0.02 * f64::from(column) + 0.015 * f64::from(row),
                0.01 * f64::from(column + row),
            ]
        }
    }

    impl PixelSource for MultiTileGrid {
        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn linear_rgb(&self, x: u32, y: u32) -> Option<Rgb> {
            Some(crate::color::apply_log_gain(
                [0.31, 0.27, 0.23],
                self.tile_gain(x, y),
            ))
        }
    }

    #[test]
    fn seam_wave_is_full_at_the_join_and_zero_at_tile_midpoints() {
        let seam = 4096;
        assert_eq!(midpoint_wave(seam - 1, seam, 0, true), 1.0);
        assert_eq!(midpoint_wave(seam, seam, 8192, false), 1.0);
        assert_eq!(midpoint_wave(2048, seam, 0, true), 0.0);
        assert_eq!(midpoint_wave(6144, seam, 8192, false), 0.0);
        assert_eq!(midpoint_wave(1024, seam, 0, true), 0.0);
        assert_eq!(midpoint_wave(7168, seam, 8192, false), 0.0);
        assert!(midpoint_wave(3072, seam, 0, true) > 0.0);
        assert!(midpoint_wave(5120, seam, 8192, false) > 0.0);
    }

    #[test]
    fn recovers_a_step_while_leaving_tile_interiors_native() {
        let source = Synthetic {
            width: 256,
            height: 128,
            right_gain: 1.1,
        };
        let config = CorrectionConfig {
            seams: SeamSpec {
                grid: Some(GridSpec {
                    columns: 2,
                    rows: 1,
                }),
                ..SeamSpec::default()
            },
            scan_radius: 6,
            sample_stride: 2,
            refine_radius: 0,
            local_strength: 0.0,
            transfer: TransferFunction::Linear,
            min_confidence: 0.05,
            ..CorrectionConfig::default()
        };
        let model = build_model(
            &source,
            &config,
            ImageReport {
                width: 256,
                height: 128,
                channels: 3,
                bit_depth: "f32".to_owned(),
                transport: "synthetic".to_owned(),
            },
        )
        .unwrap();
        // The central anchor of each tile and its outer half retain the source
        // value; the reconciled graph correction exists only as a seam wave.
        assert_eq!(model.log_gain_at(20, 64), [0.0; 3]);
        assert_eq!(model.log_gain_at(220, 64), [0.0; 3]);
        let seam = source.width / 2;
        let left = model.log_gain_at(seam - 1, 64)[0];
        let right = model.log_gain_at(seam, 64)[0];
        assert_abs_diff_eq!(right - left, -1.1_f64.ln(), epsilon = 0.01);
        assert!(model.report.boundaries[0].accepted);
        assert_eq!(model.report.field.headroom_shift_stops, 0.0);
        assert_eq!(model.report.field.neutral_interior_anchors, 2);
    }

    #[test]
    fn scanwalk_builds_a_position_varying_residual_field() {
        let source = VariableSeam {
            width: 512,
            height: 512,
        };
        let config = CorrectionConfig {
            seams: SeamSpec {
                grid: Some(GridSpec {
                    columns: 2,
                    rows: 1,
                }),
                ..SeamSpec::default()
            },
            scan_radius: 6,
            sample_stride: 1,
            refine_radius: 0,
            blend_width: 64,
            profile_smooth_radius: 0,
            strength: 1.0,
            local_strength: 1.0,
            max_gain_stops: 1.0,
            transfer: TransferFunction::Linear,
            min_confidence: 0.05,
            ..CorrectionConfig::default()
        };
        let model = build_model(
            &source,
            &config,
            ImageReport {
                width: source.width,
                height: source.height,
                channels: 3,
                bit_depth: "96-bit RGB float (32 bits/channel)".to_owned(),
                transport: "synthetic".to_owned(),
            },
        )
        .unwrap();
        assert!(model.report.boundaries[0].accepted);

        let seam = source.width / 2;
        let mut correction_differences = Vec::new();
        for y in [32, 128, 256, 384, 479] {
            let left = source.linear_rgb(seam - 1, y).unwrap();
            let right = source.linear_rgb(seam, y).unwrap();
            let left_gain = model.log_gain_at(seam - 1, y);
            let right_gain = model.log_gain_at(seam, y);
            let correction_difference = sub(right_gain, left_gain);
            correction_differences.push(correction_difference[0]);
            for channel in 0..3 {
                let corrected_jump = right[channel].ln() + right_gain[channel]
                    - left[channel].ln()
                    - left_gain[channel];
                assert_abs_diff_eq!(corrected_jump, 0.0, epsilon = 0.001);
            }
        }

        let range = correction_differences
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .unwrap()
            - correction_differences
                .iter()
                .copied()
                .min_by(f64::total_cmp)
                .unwrap();
        assert!(
            range > 0.05,
            "the correction field was unexpectedly uniform"
        );
    }

    #[test]
    fn reconciles_every_neighbor_in_a_three_by_four_grid() {
        let source = MultiTileGrid {
            width: 300,
            height: 400,
            columns: 3,
            rows: 4,
        };
        let config = CorrectionConfig {
            seams: SeamSpec {
                grid: Some(GridSpec {
                    columns: source.columns,
                    rows: source.rows,
                }),
                ..SeamSpec::default()
            },
            scan_radius: 4,
            sample_stride: 1,
            refine_radius: 0,
            blend_width: 32,
            profile_smooth_radius: 8,
            strength: 1.0,
            local_strength: 1.0,
            max_gain_stops: 1.0,
            transfer: TransferFunction::Linear,
            min_confidence: 0.05,
            ..CorrectionConfig::default()
        };
        let model = build_model(
            &source,
            &config,
            ImageReport {
                width: source.width,
                height: source.height,
                channels: 3,
                bit_depth: "96-bit RGB float (32 bits/channel)".to_owned(),
                transport: "synthetic".to_owned(),
            },
        )
        .unwrap();

        // Two vertical lines split across four rows plus three horizontal
        // lines split across three columns: 2*4 + 3*3 = 17 adjacencies.
        assert_eq!(model.report.layout.tile_count(), 12);
        assert_eq!(model.report.boundaries.len(), 17);
        assert!(model.report.boundaries.iter().all(|item| item.accepted));
        assert!(
            model.report.field.final_max_residual_stops
                <= model.report.field.initial_max_residual_stops + 1.0e-12
        );

        for x in [100, 200] {
            for y in [50, 150, 250, 350] {
                assert_corrected_neighbors_match(&source, &model, (x - 1, y), (x, y));
            }
        }
        for y in [100, 200, 300] {
            for x in [50, 150, 250] {
                assert_corrected_neighbors_match(&source, &model, (x, y - 1), (x, y));
            }
        }
    }

    fn assert_corrected_neighbors_match<S: PixelSource>(
        source: &S,
        model: &CorrectionModel,
        a: (u32, u32),
        b: (u32, u32),
    ) {
        let a_rgb = source.linear_rgb(a.0, a.1).unwrap();
        let b_rgb = source.linear_rgb(b.0, b.1).unwrap();
        let a_gain = model.log_gain_at(a.0, a.1);
        let b_gain = model.log_gain_at(b.0, b.1);
        for channel in 0..3 {
            let corrected_a = a_rgb[channel].ln() + a_gain[channel];
            let corrected_b = b_rgb[channel].ln() + b_gain[channel];
            assert_abs_diff_eq!(corrected_a, corrected_b, epsilon = 0.001);
        }
    }
}
