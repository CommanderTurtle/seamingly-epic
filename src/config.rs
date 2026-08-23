use serde::{Deserialize, Serialize};

/// An equal-sized grid used to derive seam coordinates.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GridSpec {
    pub columns: u32,
    pub rows: u32,
}

impl Default for GridSpec {
    fn default() -> Self {
        Self {
            columns: 2,
            rows: 2,
        }
    }
}

/// Either explicit boundaries or an equal grid.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SeamSpec {
    #[serde(default)]
    pub x: Vec<u32>,
    #[serde(default)]
    pub y: Vec<u32>,
    pub grid: Option<GridSpec>,
}

/// Color transfer function used when converting encoded samples to linear light.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferFunction {
    #[default]
    Srgb,
    Linear,
}

/// Complete correction settings shared by PNG and float32 transports.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct CorrectionConfig {
    pub seams: SeamSpec,
    /// Half-width of each analysis band, in output pixels.
    pub scan_radius: u32,
    /// Search this many pixels around nominal coordinates for a stronger persistent step.
    pub refine_radius: u32,
    /// Sample every Nth pixel along a boundary. One performs a complete scanline walk.
    pub sample_stride: u32,
    /// Width over which the exact-boundary closure field fades to zero.
    pub blend_width: u32,
    /// Radius of low-pass smoothing along the seam profile.
    pub profile_smooth_radius: u32,
    /// Overall correction multiplier.
    pub strength: f64,
    /// Full-resolution profile reconstruction and closure multiplier.
    pub local_strength: f64,
    /// Maximum absolute per-channel gain, expressed in photographic stops.
    pub max_gain_stops: f64,
    /// Minimum confidence needed before a constraint can affect the image.
    pub min_confidence: f64,
    /// Encoded-to-linear transfer function.
    pub transfer: TransferFunction,
    /// Number of worker threads. Zero uses Rust's global Rayon pool.
    pub threads: usize,
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        Self {
            seams: SeamSpec {
                grid: Some(GridSpec::default()),
                ..SeamSpec::default()
            },
            scan_radius: 8,
            refine_radius: 0,
            sample_stride: 1,
            blend_width: 192,
            profile_smooth_radius: 96,
            strength: 1.0,
            local_strength: 1.0,
            max_gain_stops: 0.75,
            min_confidence: 0.18,
            transfer: TransferFunction::Srgb,
            threads: 0,
        }
    }
}
