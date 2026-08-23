use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::config::SeamSpec;

/// Pixel-space boundaries and tile topology resolved for a concrete image.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub x_seams: Vec<u32>,
    pub y_seams: Vec<u32>,
}

impl Layout {
    pub fn resolve(width: u32, height: u32, spec: &SeamSpec) -> Result<Self> {
        ensure!(
            width >= 4 && height >= 4,
            "image must be at least 4x4 pixels"
        );

        let (mut x, mut y) = if let Some(grid) = spec.grid {
            ensure!(
                grid.columns > 0 && grid.rows > 0,
                "grid dimensions must be non-zero"
            );
            ensure!(
                grid.columns <= width && grid.rows <= height,
                "grid has more cells than pixels"
            );
            (
                equal_boundaries(width, grid.columns),
                equal_boundaries(height, grid.rows),
            )
        } else {
            (Vec::new(), Vec::new())
        };

        if !spec.x.is_empty() {
            x = spec.x.clone();
        }
        if !spec.y.is_empty() {
            y = spec.y.clone();
        }

        normalize_seams(&mut x, width, "x")?;
        normalize_seams(&mut y, height, "y")?;

        if x.is_empty() && y.is_empty() {
            bail!("no seams were supplied; use --grid, --x-seams, or --y-seams");
        }

        Ok(Self {
            width,
            height,
            x_seams: x,
            y_seams: y,
        })
    }

    #[must_use]
    pub fn columns(&self) -> usize {
        self.x_seams.len() + 1
    }

    #[must_use]
    pub fn rows(&self) -> usize {
        self.y_seams.len() + 1
    }

    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.columns() * self.rows()
    }

    #[must_use]
    pub fn tile_index(&self, column: usize, row: usize) -> usize {
        row * self.columns() + column
    }

    #[must_use]
    pub fn tile_at(&self, x: u32, y: u32) -> usize {
        let column = self.x_seams.partition_point(|seam| x >= *seam);
        let row = self.y_seams.partition_point(|seam| y >= *seam);
        self.tile_index(column, row)
    }

    #[must_use]
    pub fn x_edges(&self) -> Vec<u32> {
        let mut edges = Vec::with_capacity(self.x_seams.len() + 2);
        edges.push(0);
        edges.extend_from_slice(&self.x_seams);
        edges.push(self.width);
        edges
    }

    #[must_use]
    pub fn y_edges(&self) -> Vec<u32> {
        let mut edges = Vec::with_capacity(self.y_seams.len() + 2);
        edges.push(0);
        edges.extend_from_slice(&self.y_seams);
        edges.push(self.height);
        edges
    }
}

fn equal_boundaries(length: u32, cells: u32) -> Vec<u32> {
    (1..cells)
        .map(|index| {
            ((u64::from(length) * u64::from(index)) + u64::from(cells / 2)) / u64::from(cells)
        })
        .map(|value| u32::try_from(value).expect("boundary fits in u32"))
        .collect()
}

fn normalize_seams(seams: &mut Vec<u32>, limit: u32, axis: &str) -> Result<()> {
    seams.sort_unstable();
    seams.dedup();
    ensure!(
        seams.iter().all(|value| *value > 1 && *value < limit - 1),
        "{axis} seam coordinates must leave at least two pixels on both sides"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GridSpec, SeamSpec};

    #[test]
    fn resolves_two_by_two_pid_layout() {
        let layout = Layout::resolve(
            8192,
            8192,
            &SeamSpec {
                grid: Some(GridSpec {
                    columns: 2,
                    rows: 2,
                }),
                ..SeamSpec::default()
            },
        )
        .unwrap();
        assert_eq!(layout.x_seams, [4096]);
        assert_eq!(layout.y_seams, [4096]);
        assert_eq!(layout.tile_count(), 4);
        assert_eq!(layout.tile_at(4095, 4095), 0);
        assert_eq!(layout.tile_at(4096, 4095), 1);
        assert_eq!(layout.tile_at(4095, 4096), 2);
        assert_eq!(layout.tile_at(4096, 4096), 3);
    }

    #[test]
    fn supports_one_by_two_and_five_by_five() {
        let vertical = Layout::resolve(
            1000,
            2000,
            &SeamSpec {
                grid: Some(GridSpec {
                    columns: 1,
                    rows: 2,
                }),
                ..SeamSpec::default()
            },
        )
        .unwrap();
        assert!(vertical.x_seams.is_empty());
        assert_eq!(vertical.y_seams, [1000]);

        let five = Layout::resolve(
            1000,
            1000,
            &SeamSpec {
                grid: Some(GridSpec {
                    columns: 5,
                    rows: 5,
                }),
                ..SeamSpec::default()
            },
        )
        .unwrap();
        assert_eq!(five.x_seams, [200, 400, 600, 800]);
        assert_eq!(five.y_seams, [200, 400, 600, 800]);
        assert_eq!(five.tile_count(), 25);
    }

    #[test]
    fn derives_a_three_by_two_grid_from_explicit_lines() {
        let layout = Layout::resolve(
            9000,
            8192,
            &SeamSpec {
                x: vec![3084, 5887],
                y: vec![4096],
                grid: None,
            },
        )
        .unwrap();
        assert_eq!(layout.columns(), 3);
        assert_eq!(layout.rows(), 2);
        assert_eq!(layout.tile_count(), 6);
        assert_eq!(layout.x_edges(), [0, 3084, 5887, 9000]);
        assert_eq!(layout.y_edges(), [0, 4096, 8192]);
    }
}
