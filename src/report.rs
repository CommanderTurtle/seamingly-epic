use serde::{Deserialize, Serialize};

use crate::{color::Rgb, config::CorrectionConfig, layout::Layout};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BoundaryReport {
    pub orientation: Orientation,
    pub nominal_coordinate: u32,
    pub coordinate: u32,
    pub segment_index: usize,
    pub segment_start: u32,
    pub segment_end: u32,
    pub tile_a: usize,
    pub tile_b: usize,
    pub log_jump_rgb: Rgb,
    pub jump_stops_rgb: Rgb,
    pub dispersion: f64,
    pub confidence: f64,
    pub valid_samples: usize,
    pub accepted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TileGainReport {
    pub tile: usize,
    pub row: usize,
    pub column: usize,
    pub log_gain_rgb: Rgb,
    pub gain_stops_rgb: Rgb,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImageReport {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub bit_depth: String,
    pub transport: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FieldReport {
    /// Exact reconstruction method used for the full-resolution field.
    pub strategy: String,
    /// Numeric precision of the reconstructed correction field.
    pub precision: String,
    /// Number of per-position seam impulses supplied to the global solve.
    pub seam_impulses: u64,
    /// Ordered tile relationships reconciled through the sparse tile Laplacian.
    pub conceptual_tile_relationships: u64,
    /// Number of output pixels receiving independently evaluated corrections.
    pub output_pixels: u64,
    /// Storage occupied by target and two-sided f64 RGB seam profiles.
    pub stored_field_bytes: u64,
    /// Retained for report compatibility. Midpoint anchoring keeps this zero.
    pub headroom_shift_stops: f64,
    /// Number of tile interiors at which every normal correction wave is zero.
    #[serde(default)]
    pub neutral_interior_anchors: u64,
    /// Accepted alternating projection passes across intersecting seam profiles.
    #[serde(default)]
    pub refinement_passes: u32,
    /// Maximum boundary mismatch before intersection refinement, in stops.
    #[serde(default)]
    pub initial_max_residual_stops: f64,
    /// Maximum boundary mismatch after intersection refinement, in stops.
    #[serde(default)]
    pub final_max_residual_stops: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CorrectionReport {
    pub version: u32,
    pub image: ImageReport,
    pub layout: Layout,
    pub config: CorrectionConfig,
    pub boundaries: Vec<BoundaryReport>,
    pub tile_gains: Vec<TileGainReport>,
    pub field: FieldReport,
    pub warnings: Vec<String>,
    pub applied: bool,
}
