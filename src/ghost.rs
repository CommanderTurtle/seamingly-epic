use std::{f64::consts::PI, fs::File};

use anyhow::{Context, Result, ensure};
use memmap2::{Mmap, MmapMut, MmapOptions};
use rayon::prelude::*;
use rustdct::{DctPlanner, TransformType2And3};

use crate::{color::Rgb, scratch::temporary_file};

/// A full-resolution, three-channel correction field backed by a temporary
/// memory map. The planes are f64 because the field is the only transformed
/// image-sized quantity; source samples remain in their original transport.
pub(crate) struct GhostField {
    _file: File,
    map: Mmap,
    width: usize,
    height: usize,
    plane_bytes: usize,
}

impl GhostField {
    pub(crate) fn sample(&self, x: u32, y: u32) -> Rgb {
        let x = x as usize;
        let y = y as usize;
        debug_assert!(x < self.width && y < self.height);
        let pixel = y * self.width + x;
        [
            self.read_channel(0, pixel),
            self.read_channel(1, pixel),
            self.read_channel(2, pixel),
        ]
    }

    fn read_channel(&self, channel: usize, pixel: usize) -> f64 {
        let start = channel * self.plane_bytes + pixel * size_of::<f64>();
        f64::from_ne_bytes(
            self.map[start..start + size_of::<f64>()]
                .try_into()
                .expect("validated ghost-field byte range"),
        )
    }
}

pub(crate) struct GhostFieldBuilder {
    file: File,
    map: MmapMut,
    width: usize,
    height: usize,
    pixel_count: usize,
    plane_bytes: usize,
}

impl GhostFieldBuilder {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self> {
        let width = width as usize;
        let height = height as usize;
        let pixel_count = width
            .checked_mul(height)
            .context("ghost field exceeds this platform's address space")?;
        let plane_bytes = pixel_count
            .checked_mul(size_of::<f64>())
            .context("ghost-field plane exceeds this platform's address space")?;
        let total_bytes = plane_bytes
            .checked_mul(3)
            .context("RGB ghost field exceeds this platform's address space")?;
        let file = temporary_file("ghost-field store")?;
        file.set_len(total_bytes as u64)
            .context("could not size temporary ghost-field store")?;
        let map = map_temporary_file(&file, total_bytes)?;
        Ok(Self {
            file,
            map,
            width,
            height,
            pixel_count,
            plane_bytes,
        })
    }

    pub(crate) fn write_channel(&mut self, channel: usize, values: &[f64]) -> Result<()> {
        ensure!(channel < 3, "ghost-field channel must be RGB");
        ensure!(
            values.len() == self.pixel_count,
            "ghost-field solution has an unexpected pixel count"
        );
        let start = channel * self.plane_bytes;
        let plane = &mut self.map[start..start + self.plane_bytes];
        plane
            .par_chunks_mut(size_of::<f64>())
            .zip(values.par_iter())
            .for_each(|(destination, value)| destination.copy_from_slice(&value.to_ne_bytes()));
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<GhostField> {
        self.map
            .flush()
            .context("could not flush temporary ghost-field store")?;
        let map = self
            .map
            .make_read_only()
            .context("could not make the ghost field read-only")?;
        Ok(GhostField {
            _file: self.file,
            map,
            width: self.width,
            height: self.height,
            plane_bytes: self.plane_bytes,
        })
    }
}

/// Solve `L h = rhs` on the complete pixel grid with zero-flux (Neumann)
/// outer boundaries and zero mean. DCT-II diagonalizes this exact discrete
/// graph Laplacian, so the result is the sum of every seam impulse's Green's
/// function without materializing one full image for each conceptual impulse.
pub(crate) fn solve_neumann_poisson(
    width: u32,
    height: u32,
    mut rhs: Vec<f64>,
) -> Result<Vec<f64>> {
    let width = width as usize;
    let height = height as usize;
    let pixel_count = width
        .checked_mul(height)
        .context("Poisson field exceeds this platform's address space")?;
    ensure!(
        rhs.len() == pixel_count,
        "Poisson right-hand side has an unexpected pixel count"
    );
    ensure!(
        width > 1 && height > 1,
        "Poisson field must be at least 2x2"
    );

    // A pure Neumann Laplacian has a one-dimensional nullspace. Incidence
    // forces sum to zero analytically; removing floating-point drift makes the
    // gauge explicit and prevents energy from leaking into the DC mode.
    let mean = rhs.par_iter().sum::<f64>() / pixel_count as f64;
    rhs.par_iter_mut().for_each(|value| *value -= mean);

    let mut planner = DctPlanner::<f64>::new();
    let row_transform = planner.plan_dct2(width);
    let column_transform = planner.plan_dct2(height);

    transform_rows(
        &mut rhs,
        width,
        row_transform.as_ref(),
        TransformDirection::Forward,
    );
    let mut transposed = transpose(&rhs, width, height)?;
    transform_rows(
        &mut transposed,
        height,
        column_transform.as_ref(),
        TransformDirection::Forward,
    );
    rhs = transpose(&transposed, height, width)?;
    drop(transposed);

    let x_eigenvalues: Vec<f64> = (0..width)
        .map(|index| 2.0 - 2.0 * (PI * index as f64 / width as f64).cos())
        .collect();
    let y_eigenvalues: Vec<f64> = (0..height)
        .map(|index| 2.0 - 2.0 * (PI * index as f64 / height as f64).cos())
        .collect();
    rhs.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            let eigenvalue = x_eigenvalues[x] + y_eigenvalues[y];
            *value = if x == 0 && y == 0 {
                0.0
            } else {
                *value / eigenvalue
            };
        }
    });

    transform_rows(
        &mut rhs,
        width,
        row_transform.as_ref(),
        TransformDirection::Inverse,
    );
    let mut transposed = transpose(&rhs, width, height)?;
    transform_rows(
        &mut transposed,
        height,
        column_transform.as_ref(),
        TransformDirection::Inverse,
    );
    rhs = transpose(&transposed, height, width)?;

    // rustdct deliberately leaves transforms unnormalized. DCT-III(DCT-II(x))
    // equals N/2*x on each axis, so the exact 2-D inverse factor is 4/(W*H).
    let inverse_scale = 4.0 / pixel_count as f64;
    rhs.par_iter_mut().for_each(|value| *value *= inverse_scale);
    ensure!(
        rhs.par_iter().all(|value| value.is_finite()),
        "Poisson reconstruction produced a non-finite correction"
    );
    Ok(rhs)
}

enum TransformDirection {
    Forward,
    Inverse,
}

fn transform_rows(
    values: &mut [f64],
    row_length: usize,
    transform: &dyn TransformType2And3<f64>,
    direction: TransformDirection,
) {
    values.par_chunks_mut(row_length).for_each_init(
        || vec![0.0; transform.get_scratch_len()],
        |scratch, row| match direction {
            TransformDirection::Forward => {
                transform.process_dct2_with_scratch(row, scratch);
            }
            TransformDirection::Inverse => {
                transform.process_dct3_with_scratch(row, scratch);
            }
        },
    );
}

fn transpose(input: &[f64], width: usize, height: usize) -> Result<Vec<f64>> {
    ensure!(
        input.len() == width * height,
        "transpose dimensions do not match the input"
    );
    let mut output = try_zeroed(input.len())?;
    output
        .par_chunks_mut(height)
        .enumerate()
        .for_each(|(x, column)| {
            for (y, value) in column.iter_mut().enumerate() {
                *value = input[y * width + x];
            }
        });
    Ok(output)
}

pub(crate) fn try_zeroed(length: usize) -> Result<Vec<f64>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .context("could not reserve the full-resolution f64 correction field")?;
    values.resize(length, 0.0);
    Ok(values)
}

#[allow(unsafe_code)]
fn map_temporary_file(file: &File, length: usize) -> Result<MmapMut> {
    // SAFETY: the file is a new, exclusively owned temporary store. Its length
    // is fixed before mapping and remains fixed for the map's lifetime.
    unsafe { MmapOptions::new().len(length).map_mut(file) }
        .context("could not memory-map temporary ghost-field store")
}
