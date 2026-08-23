use std::f64::consts::{LN_2, PI};

use anyhow::{Context, Result, ensure};
use rayon::prelude::*;

use crate::{
    color::{Rgb, add, log_gain_to_stops, log_rgb, norm, scale, sub},
    config::CorrectionConfig,
    ghost::{GhostField, GhostFieldBuilder, solve_neumann_poisson, try_zeroed},
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
        self.estimate.is_some() && self.confidence >= config.min_confidence
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
    profile: Vec<Rgb>,
}

impl LocalField {
    fn sample(&self, position: u32) -> Rgb {
        if self.profile.is_empty() {
            return [0.0; 3];
        }
        let index = position
            .saturating_sub(self.segment_start)
            .min(self.segment_end - self.segment_start - 1);
        self.profile[index as usize]
    }
}

/// Fully analyzed correction field. PNG and float32 transports share it.
pub(crate) struct CorrectionModel {
    pub report: CorrectionReport,
    gains: Vec<Rgb>,
    ghost: Option<GhostField>,
    vertical: Vec<Option<LocalField>>,
    horizontal: Vec<Option<LocalField>>,
    max_log_gain: f64,
    headroom_log_offset: f64,
}

impl CorrectionModel {
    #[must_use]
    pub fn log_gain_at(&self, x: u32, y: u32) -> Rgb {
        let mut gain = self.unshifted_log_gain_at(x, y);
        for channel in &mut gain {
            *channel += self.headroom_log_offset;
        }
        gain
    }

    fn unshifted_log_gain_at(&self, x: u32, y: u32) -> Rgb {
        let layout = &self.report.layout;
        let column = layout.x_seams.partition_point(|seam| x >= *seam);
        let row = layout.y_seams.partition_point(|seam| y >= *seam);
        let tile = layout.tile_index(column, row);
        let mut gain = self.gains[tile];
        let blend_width = self.report.config.blend_width;
        let local_strength = self.report.config.local_strength.clamp(0.0, 2.0);

        if local_strength > 0.0
            && let Some(field) = &self.ghost
        {
            gain = add(gain, scale(field.sample(x, y), local_strength));
        }

        if blend_width > 0 && local_strength > 0.0 {
            // Only the immediately adjacent boundaries can influence a pixel when
            // ordinary non-overlapping tile widths are used. Checking both sides
            // also remains correct for deliberately narrow or irregular grids.
            for seam_index in [column.checked_sub(1), Some(column)]
                .into_iter()
                .flatten()
                .filter(|index| *index < layout.x_seams.len())
            {
                let field_index = seam_index * layout.rows() + row;
                if let Some(field) = &self.vertical[field_index] {
                    let delta = i64::from(x) - i64::from(field.coordinate);
                    let distance = delta.unsigned_abs();
                    if distance < u64::from(blend_width) {
                        let fade = raised_cosine(distance as f64 / f64::from(blend_width));
                        let side = if delta < 0 { 0.5 } else { -0.5 };
                        // Confidence gates creation of the field. Attenuating an
                        // accepted residual by confidence would intentionally
                        // leave a measurable fraction of the seam in place.
                        let amount = side * fade * local_strength;
                        gain = add(gain, scale(field.sample(y), amount));
                    }
                }
            }

            for seam_index in [row.checked_sub(1), Some(row)]
                .into_iter()
                .flatten()
                .filter(|index| *index < layout.y_seams.len())
            {
                let field_index = seam_index * layout.columns() + column;
                if let Some(field) = &self.horizontal[field_index] {
                    let delta = i64::from(y) - i64::from(field.coordinate);
                    let distance = delta.unsigned_abs();
                    if distance < u64::from(blend_width) {
                        let fade = raised_cosine(distance as f64 / f64::from(blend_width));
                        let side = if delta < 0 { 0.5 } else { -0.5 };
                        let amount = side * fade * local_strength;
                        gain = add(gain, scale(field.sample(x), amount));
                    }
                }
            }
        }

        gain.map(|value| value.clamp(-self.max_log_gain, self.max_log_gain))
    }
}

fn raised_cosine(unit_distance: f64) -> f64 {
    if unit_distance >= 1.0 {
        0.0
    } else {
        0.5 * (1.0 + (PI * unit_distance.max(0.0)).cos())
    }
}

fn build_global_ghost(
    layout: &Layout,
    vertical: &[Option<LocalField>],
    horizontal: &[Option<LocalField>],
) -> Result<GhostField> {
    let width = layout.width as usize;
    let height = layout.height as usize;
    let pixel_count = width
        .checked_mul(height)
        .context("global correction field exceeds this platform's address space")?;
    let mut store = GhostFieldBuilder::new(layout.width, layout.height)?;

    for channel in 0..3 {
        let mut rhs = try_zeroed(pixel_count)?;
        for field in vertical.iter().flatten() {
            let right_x = field.coordinate as usize;
            let left_x = right_x - 1;
            for (offset, profile) in field.profile.iter().enumerate() {
                let y = field.segment_start as usize + offset;
                let left = y * width + left_x;
                let right = y * width + right_x;
                // Desired edge gradient is -profile. For incidence [-1,+1],
                // B^T(-profile) contributes +profile left and -profile right.
                rhs[left] += profile[channel];
                rhs[right] -= profile[channel];
            }
        }
        for field in horizontal.iter().flatten() {
            let bottom_y = field.coordinate as usize;
            let top_y = bottom_y - 1;
            for (offset, profile) in field.profile.iter().enumerate() {
                let x = field.segment_start as usize + offset;
                let top = top_y * width + x;
                let bottom = bottom_y * width + x;
                rhs[top] += profile[channel];
                rhs[bottom] -= profile[channel];
            }
        }
        let solution = solve_neumann_poisson(layout.width, layout.height, rhs)?;
        store.write_channel(channel, &solution)?;
    }
    store.finish()
}

fn close_projected_residuals(
    ghost: &GhostField,
    vertical: &mut [Option<LocalField>],
    horizontal: &mut [Option<LocalField>],
    config: &CorrectionConfig,
) {
    if config.blend_width == 0 {
        return;
    }
    // At the two discrete samples straddling a seam, the symmetric ramp has
    // coefficients +0.5*cosine(1/w) and -0.5. Compensating by their exact
    // difference closes whatever non-integrable component the global least-
    // squares projection could not reproduce.
    let closure_factor = 0.5 * (1.0 + raised_cosine(1.0 / f64::from(config.blend_width)));

    for field in vertical.iter_mut().flatten() {
        for (offset, profile) in field.profile.iter_mut().enumerate() {
            let y = field.segment_start + offset as u32;
            let left = ghost.sample(field.coordinate - 1, y);
            let right = ghost.sample(field.coordinate, y);
            let achieved = sub(right, left);
            *profile = scale(add(*profile, achieved), 1.0 / closure_factor);
        }
    }
    for field in horizontal.iter_mut().flatten() {
        for (offset, profile) in field.profile.iter_mut().enumerate() {
            let x = field.segment_start + offset as u32;
            let top = ghost.sample(x, field.coordinate - 1);
            let bottom = ghost.sample(x, field.coordinate);
            let achieved = sub(bottom, top);
            *profile = scale(add(*profile, achieved), 1.0 / closure_factor);
        }
    }
}

fn compute_headroom_offset<S: PixelSource>(source: &S, model: &CorrectionModel) -> f64 {
    let corrected_peak = (0..source.height())
        .into_par_iter()
        .map(|y| {
            let mut row_peak = 0.0_f64;
            for x in 0..source.width() {
                let Some(rgb) = source.linear_rgb(x, y) else {
                    continue;
                };
                let gain = model.unshifted_log_gain_at(x, y);
                for channel in 0..3 {
                    row_peak = row_peak.max(rgb[channel] * gain[channel].exp());
                }
            }
            row_peak
        })
        .reduce(|| 0.0, f64::max);
    if corrected_peak > 1.0 && corrected_peak.is_finite() {
        -corrected_peak.ln()
    } else {
        0.0
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
        // Applying a component-wide gain when its neighboring boundary was
        // rejected could create a new seam. Disconnected analyses therefore
        // use only the bounded local residual field around accepted joins.
        vec![[0.0; 3]; layout.tile_count()]
    };
    limit_gains(&mut gains, config.strength, config.max_gain_stops);

    let mut vertical = vec![None; layout.x_seams.len() * layout.rows()];
    let mut horizontal = vec![None; layout.y_seams.len() * layout.columns()];
    for analysis in &analyses {
        if !analysis.accepted(config) {
            continue;
        }
        let global_difference = sub(gains[analysis.tile_b], gains[analysis.tile_a]);
        let field = make_local_field(analysis, global_difference, config);
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
        .map(|field| field.profile.len() as u64)
        .sum::<u64>();
    let ghost = if accepted > 0 && config.local_strength > 0.0 {
        let field = build_global_ghost(&layout, &vertical, &horizontal)?;
        close_projected_residuals(&field, &mut vertical, &mut horizontal, config);
        Some(field)
    } else {
        None
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
            "Accepted constraints do not connect every tile; global tile gains were disabled while accepted full-resolution seam constraints remained active."
                .to_owned(),
        );
    }
    if config.blend_width > 0 {
        let x_edges = layout.x_edges();
        let y_edges = layout.y_edges();
        let narrowest = x_edges
            .windows(2)
            .chain(y_edges.windows(2))
            .map(|edge| edge[1] - edge[0])
            .min()
            .unwrap_or(0);
        if config.blend_width.saturating_mul(2) > narrowest {
            warnings.push(
                "Local correction ramps overlap because blend width exceeds half a tile; the combined gain remains clamped."
                    .to_owned(),
            );
        }
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
        version: 2,
        image,
        layout,
        config: config.clone(),
        boundaries,
        tile_gains,
        field: FieldReport {
            strategy: if ghost.is_some() {
                "global_neumann_poisson_dct_with_exact_boundary_closure".to_owned()
            } else {
                "disabled".to_owned()
            },
            precision: "f64_log_linear_rgb".to_owned(),
            seam_impulses,
            conceptual_tile_relationships: tile_count.saturating_mul(tile_count),
            output_pixels,
            stored_field_bytes: if ghost.is_some() {
                output_pixels.saturating_mul(3 * size_of::<f64>() as u64)
            } else {
                0
            },
            headroom_shift_stops: 0.0,
        },
        warnings,
        applied: accepted > 0,
    };

    let mut model = CorrectionModel {
        report,
        gains,
        ghost,
        vertical,
        horizontal,
        max_log_gain: config.max_gain_stops.max(0.0) * LN_2,
        headroom_log_offset: 0.0,
    };
    if model.report.applied {
        model.headroom_log_offset = with_threads(config.threads, || {
            Ok(compute_headroom_offset(source, &model))
        })?;
        model.report.field.headroom_shift_stops = model.headroom_log_offset / LN_2;
    }
    Ok(model)
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
    global_difference: Rgb,
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
            add(stabilized, global_difference),
        ));
    }

    let fallback = add(estimate.center, global_difference);
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

    LocalField {
        coordinate: analysis.coordinate,
        segment_start: analysis.segment_start,
        segment_end: analysis.segment_end,
        profile,
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
    fn recovers_a_step_without_mistaking_a_linear_gradient_for_it() {
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
        let left = model.log_gain_at(20, 64)[0];
        let right = model.log_gain_at(220, 64)[0];
        assert_abs_diff_eq!(right - left, -1.1_f64.ln(), epsilon = 0.01);
        assert!(model.report.boundaries[0].accepted);
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
