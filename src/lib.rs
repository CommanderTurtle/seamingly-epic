//! Native photometric seam correction for independently generated image tiles.

pub mod color;
pub mod config;
pub mod engine;
pub mod layout;
pub mod png_io;
pub mod raw_io;
pub mod report;
pub mod robust;
mod scratch;
pub mod solve;

pub use config::{CorrectionConfig, GridSpec, SeamSpec};
pub use png_io::{analyze_png, correct_png};
pub use raw_io::correct_raw_f32;
pub use report::CorrectionReport;
