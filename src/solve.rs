use anyhow::{Result, ensure};

use crate::color::{Rgb, scale};

#[derive(Clone, Copy, Debug)]
pub struct GainConstraint {
    pub left_or_top: usize,
    pub right_or_bottom: usize,
    pub jump: Rgb,
    pub weight: f64,
}

/// Solve `gain[b] - gain[a] = -jump` for all tile adjacencies.
#[allow(clippy::needless_range_loop)]
pub fn solve_tile_gains(tile_count: usize, constraints: &[GainConstraint]) -> Result<Vec<Rgb>> {
    ensure!(tile_count > 0, "tile graph is empty");
    if tile_count == 1 || constraints.is_empty() {
        return Ok(vec![[0.0; 3]; tile_count]);
    }

    let mut result = vec![[0.0; 3]; tile_count];
    for component in connected_components(tile_count, constraints) {
        if component.len() == 1 {
            continue;
        }
        let component_constraints: Vec<&GainConstraint> = constraints
            .iter()
            .filter(|constraint| {
                component.contains(&constraint.left_or_top)
                    && component.contains(&constraint.right_or_bottom)
            })
            .collect();
        for channel in 0..3 {
            let count = component.len();
            let mut matrix = vec![vec![0.0; count]; count];
            let mut rhs = vec![0.0; count];
            for constraint in &component_constraints {
                let a = component
                    .iter()
                    .position(|tile| *tile == constraint.left_or_top)
                    .expect("constraint endpoint belongs to component");
                let b = component
                    .iter()
                    .position(|tile| *tile == constraint.right_or_bottom)
                    .expect("constraint endpoint belongs to component");
                let weight = constraint.weight.max(0.0);
                matrix[a][a] += weight;
                matrix[b][b] += weight;
                matrix[a][b] -= weight;
                matrix[b][a] -= weight;
                rhs[a] += weight * constraint.jump[channel];
                rhs[b] -= weight * constraint.jump[channel];
            }

            // Fix the otherwise free exposure of this connected component.
            let anchor_weight = component_constraints
                .iter()
                .map(|item| item.weight)
                .sum::<f64>()
                .max(1.0);
            matrix[0][0] += anchor_weight;

            let solved = gaussian_solve(matrix, rhs)?;
            let mean = solved.iter().sum::<f64>() / count as f64;
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

fn gaussian_solve(mut matrix: Vec<Vec<f64>>, mut rhs: Vec<f64>) -> Result<Vec<f64>> {
    let n = rhs.len();
    for pivot in 0..n {
        let best = (pivot..n)
            .max_by(|left, right| {
                matrix[*left][pivot]
                    .abs()
                    .total_cmp(&matrix[*right][pivot].abs())
            })
            .expect("pivot range is non-empty");
        ensure!(
            matrix[best][pivot].abs() > 1.0e-12,
            "tile constraint graph is disconnected"
        );
        matrix.swap(pivot, best);
        rhs.swap(pivot, best);

        let divisor = matrix[pivot][pivot];
        for value in &mut matrix[pivot][pivot..] {
            *value /= divisor;
        }
        rhs[pivot] /= divisor;
        let pivot_row = matrix[pivot].clone();

        for row in 0..n {
            if row == pivot {
                continue;
            }
            let factor = matrix[row][pivot];
            if factor.abs() <= 1.0e-18 {
                continue;
            }
            for (column, value) in matrix[row].iter_mut().enumerate().skip(pivot) {
                *value -= factor * pivot_row[column];
            }
            rhs[row] -= factor * rhs[pivot];
        }
    }
    Ok(rhs)
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
