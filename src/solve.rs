use anyhow::{Result, bail, ensure};

use crate::color::{Rgb, scale};

#[derive(Clone, Copy, Debug)]
pub struct GainConstraint {
    pub left_or_top: usize,
    pub right_or_bottom: usize,
    pub jump: Rgb,
    pub weight: f64,
}

#[derive(Clone, Copy, Debug)]
struct LocalConstraint {
    a: usize,
    b: usize,
    jump: Rgb,
    weight: f64,
}

/// Solve `gain[b] - gain[a] = -jump` for all tile adjacencies.
pub fn solve_tile_gains(tile_count: usize, constraints: &[GainConstraint]) -> Result<Vec<Rgb>> {
    ensure!(tile_count > 0, "tile graph is empty");
    for constraint in constraints {
        ensure!(
            constraint.left_or_top < tile_count && constraint.right_or_bottom < tile_count,
            "tile constraint endpoint is outside the graph"
        );
        ensure!(
            constraint.weight.is_finite() && constraint.weight >= 0.0,
            "tile constraint weight must be finite and non-negative"
        );
        ensure!(
            constraint.jump.iter().all(|value| value.is_finite()),
            "tile constraint jump must be finite"
        );
    }
    if tile_count == 1 || constraints.is_empty() {
        return Ok(vec![[0.0; 3]; tile_count]);
    }

    let mut result = vec![[0.0; 3]; tile_count];
    for component in connected_components(tile_count, constraints) {
        if component.len() == 1 {
            continue;
        }
        let mut local_index = vec![usize::MAX; tile_count];
        for (index, tile) in component.iter().copied().enumerate() {
            local_index[tile] = index;
        }
        let component_constraints: Vec<LocalConstraint> = constraints
            .iter()
            .filter_map(|constraint| {
                let a = local_index[constraint.left_or_top];
                let b = local_index[constraint.right_or_bottom];
                (a != usize::MAX && b != usize::MAX && constraint.weight > 0.0).then_some(
                    LocalConstraint {
                        a,
                        b,
                        jump: constraint.jump,
                        weight: constraint.weight,
                    },
                )
            })
            .collect();
        for channel in [0_usize, 1, 2] {
            let solved = solve_component_channel(component.len(), &component_constraints, channel)?;
            let mean = solved.iter().sum::<f64>() / component.len() as f64;
            for (index, value) in solved.into_iter().enumerate() {
                result[component[index]][channel] = value - mean;
            }
        }
    }
    Ok(result)
}

fn connected_components(tile_count: usize, constraints: &[GainConstraint]) -> Vec<Vec<usize>> {
    let mut adjacency = vec![Vec::new(); tile_count];
    for constraint in constraints.iter().filter(|item| item.weight > 0.0) {
        adjacency[constraint.left_or_top].push(constraint.right_or_bottom);
        adjacency[constraint.right_or_bottom].push(constraint.left_or_top);
    }
    let mut seen = vec![false; tile_count];
    let mut components = Vec::new();
    for start in 0..tile_count {
        if seen[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        seen[start] = true;
        while let Some(tile) = stack.pop() {
            component.push(tile);
            for neighbor in &adjacency[tile] {
                if !seen[*neighbor] {
                    seen[*neighbor] = true;
                    stack.push(*neighbor);
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    components
}

/// Solve one channel of the anchored weighted graph Laplacian with
/// Jacobi-preconditioned conjugate gradients. The matrix is never materialized:
/// storage and each iteration are O(tiles + shared edges), so large grids do not
/// acquire the dense O(tiles²) memory cost of Gaussian elimination.
fn solve_component_channel(
    tile_count: usize,
    constraints: &[LocalConstraint],
    channel: usize,
) -> Result<Vec<f64>> {
    let anchor_weight = constraints
        .iter()
        .map(|item| item.weight)
        .sum::<f64>()
        .max(1.0);
    let mut diagonal = vec![0.0; tile_count];
    let mut rhs = vec![0.0; tile_count];
    for constraint in constraints {
        diagonal[constraint.a] += constraint.weight;
        diagonal[constraint.b] += constraint.weight;
        rhs[constraint.a] += constraint.weight * constraint.jump[channel];
        rhs[constraint.b] -= constraint.weight * constraint.jump[channel];
    }
    diagonal[0] += anchor_weight;
    ensure!(
        diagonal
            .iter()
            .all(|value| value.is_finite() && *value > 0.0),
        "tile constraint graph is disconnected"
    );
    ensure!(
        rhs.iter().all(|value| value.is_finite()),
        "global tile solver right-hand side overflowed"
    );

    let rhs_norm = dot(&rhs, &rhs).sqrt();
    ensure!(rhs_norm.is_finite(), "global tile solver norm overflowed");
    if rhs_norm <= f64::EPSILON {
        return Ok(vec![0.0; tile_count]);
    }

    let mut solution = vec![0.0; tile_count];
    let mut residual = rhs;
    let mut preconditioned: Vec<f64> = residual
        .iter()
        .zip(&diagonal)
        .map(|(value, divisor)| value / divisor)
        .collect();
    let mut direction = preconditioned.clone();
    let mut residual_dot_preconditioned = dot(&residual, &preconditioned);
    let tolerance = (rhs_norm * 1.0e-11).max(1.0e-13);
    let max_iterations = tile_count.saturating_mul(32).clamp(256, 1_000_000);

    for _ in 0..max_iterations {
        let product = laplacian_multiply(&direction, constraints, anchor_weight);
        let denominator = dot(&direction, &product);
        if !denominator.is_finite() || denominator <= 1.0e-24 {
            bail!("global tile solver encountered a non-positive graph direction");
        }
        let alpha = residual_dot_preconditioned / denominator;
        for index in 0..tile_count {
            solution[index] += alpha * direction[index];
            residual[index] -= alpha * product[index];
        }
        if dot(&residual, &residual).sqrt() <= tolerance {
            return Ok(solution);
        }

        for index in 0..tile_count {
            preconditioned[index] = residual[index] / diagonal[index];
        }
        let next_dot = dot(&residual, &preconditioned);
        ensure!(
            next_dot.is_finite() && residual_dot_preconditioned > 0.0,
            "global tile solver produced a non-finite residual"
        );
        let beta = next_dot / residual_dot_preconditioned;
        for index in 0..tile_count {
            direction[index] = preconditioned[index] + beta * direction[index];
        }
        residual_dot_preconditioned = next_dot;
    }

    bail!("global tile solver did not converge for a {tile_count}-tile connected component")
}

fn laplacian_multiply(
    input: &[f64],
    constraints: &[LocalConstraint],
    anchor_weight: f64,
) -> Vec<f64> {
    let mut output = vec![0.0; input.len()];
    for constraint in constraints {
        let difference = input[constraint.a] - input[constraint.b];
        let weighted = constraint.weight * difference;
        output[constraint.a] += weighted;
        output[constraint.b] -= weighted;
    }
    output[0] += anchor_weight * input[0];
    output
}

fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

pub fn limit_gains(gains: &mut [Rgb], strength: f64, max_stops: f64) {
    let max_log = max_stops.max(0.0) * std::f64::consts::LN_2;
    for gain in gains {
        *gain = scale(*gain, strength.clamp(0.0, 2.0));
        for channel in gain {
            *channel = channel.clamp(-max_log, max_log);
        }
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_abs_diff_eq;

    use super::*;

    #[test]
    fn closes_a_two_by_two_gain_cycle() {
        let constraints = [
            GainConstraint {
                left_or_top: 0,
                right_or_bottom: 1,
                jump: [0.2; 3],
                weight: 1.0,
            },
            GainConstraint {
                left_or_top: 2,
                right_or_bottom: 3,
                jump: [0.2; 3],
                weight: 1.0,
            },
            GainConstraint {
                left_or_top: 0,
                right_or_bottom: 2,
                jump: [-0.1; 3],
                weight: 1.0,
            },
            GainConstraint {
                left_or_top: 1,
                right_or_bottom: 3,
                jump: [-0.1; 3],
                weight: 1.0,
            },
        ];
        let gains = solve_tile_gains(4, &constraints).unwrap();
        assert_abs_diff_eq!(gains[1][0] - gains[0][0], -0.2, epsilon = 1.0e-9);
        assert_abs_diff_eq!(gains[2][0] - gains[0][0], 0.1, epsilon = 1.0e-9);
        assert_abs_diff_eq!(
            gains.iter().map(|value| value[0]).sum::<f64>(),
            0.0,
            epsilon = 1.0e-9
        );
    }

    #[test]
    fn leaves_a_disconnected_tile_unchanged() {
        let constraints = [GainConstraint {
            left_or_top: 0,
            right_or_bottom: 1,
            jump: [0.2; 3],
            weight: 1.0,
        }];
        let gains = solve_tile_gains(3, &constraints).unwrap();
        assert_abs_diff_eq!(gains[1][0] - gains[0][0], -0.2, epsilon = 1.0e-9);
        assert_eq!(gains[2], [0.0; 3]);
    }
}
