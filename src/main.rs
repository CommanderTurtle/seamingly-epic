use std::{fs, path::PathBuf};

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use seamingly_epic::{
    CorrectionConfig, GridSpec, SeamSpec, analyze_png, correct_png, correct_raw_f32,
    report::CorrectionReport,
};

#[derive(Parser)]
#[command(
    name = "seamingly-epic",
    version,
    about = "Correct straight photometric boundaries between independently refined image tiles",
    long_about = "Bounded-memory, lossless-PNG seam analysis and correction. The engine changes only a smooth exposure/white-balance field; it never resamples or spatially filters source detail."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Measure boundaries and emit the proposed correction without changing pixels.
    Analyze {
        input: PathBuf,
        #[command(flatten)]
        settings: Settings,
        /// Write JSON to this file instead of standard output.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Correct a PNG and preserve its bit depth, alpha, and recognized metadata.
    Correct {
        input: PathBuf,
        output: PathBuf,
        #[command(flatten)]
        settings: Settings,
        /// Replace an existing output after the new PNG has encoded successfully.
        #[arg(long)]
        overwrite: bool,
        /// Also save the complete JSON analysis report.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Process a little-endian [B,H,W,C] float32 descriptor (used by ComfyUI).
    RawF32 {
        /// JSON descriptor containing paths, dimensions, channels, and settings.
        descriptor: PathBuf,
    },
}

#[derive(Clone, Debug, Args)]
struct Settings {
    /// Equal grid shorthand, such as 2x2, 1x2, or 5x5.
    #[arg(long, value_parser = parse_grid, default_value = "2x2")]
    grid: GridSpec,
    /// Ignore grid-derived coordinates; useful with only explicit seam lists.
    #[arg(long)]
    no_grid: bool,
    /// Explicit vertical boundaries, in output pixels (comma separated).
    #[arg(long, value_delimiter = ',')]
    x_seams: Vec<u32>,
    /// Explicit horizontal boundaries, in output pixels (comma separated).
    #[arg(long, value_delimiter = ',')]
    y_seams: Vec<u32>,
    /// Half-width of each analysis band.
    #[arg(long, default_value_t = 8)]
    scan_radius: u32,
    /// Search this many pixels around each nominal boundary.
    #[arg(long, default_value_t = 2)]
    refine_radius: u32,
    /// Sample every Nth pixel along a boundary.
    #[arg(long, default_value_t = 4)]
    sample_stride: u32,
    /// Width of the raised-cosine local residual ramp.
    #[arg(long, default_value_t = 192)]
    blend_width: u32,
    /// Low-pass radius along a residual profile (source pixels are never blurred).
    #[arg(long, default_value_t = 96)]
    profile_smooth_radius: u32,
    /// Global correction multiplier.
    #[arg(long, default_value_t = 1.0)]
    strength: f64,
    /// Local residual correction multiplier.
    #[arg(long, default_value_t = 0.65)]
    local_strength: f64,
    /// Per-channel correction limit in photographic stops.
    #[arg(long, default_value_t = 0.75)]
    max_gain_stops: f64,
    /// Reject boundary segments below this confidence.
    #[arg(long, default_value_t = 0.18)]
    min_confidence: f64,
    /// Transfer function of the stored RGB values.
    #[arg(long, value_enum, default_value_t = TransferArg::Srgb)]
    transfer: TransferArg,
    /// Worker count; zero uses Rayon's platform default.
    #[arg(long, default_value_t = 0)]
    threads: usize,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TransferArg {
    Srgb,
    Linear,
}

impl Settings {
    fn config(self) -> CorrectionConfig {
        CorrectionConfig {
            seams: SeamSpec {
                x: self.x_seams,
                y: self.y_seams,
                grid: (!self.no_grid).then_some(self.grid),
            },
            scan_radius: self.scan_radius,
            refine_radius: self.refine_radius,
            sample_stride: self.sample_stride,
            blend_width: self.blend_width,
            profile_smooth_radius: self.profile_smooth_radius,
            strength: self.strength,
            local_strength: self.local_strength,
            max_gain_stops: self.max_gain_stops,
            min_confidence: self.min_confidence,
            transfer: match self.transfer {
                TransferArg::Srgb => seamingly_epic::config::TransferFunction::Srgb,
                TransferArg::Linear => seamingly_epic::config::TransferFunction::Linear,
            },
            threads: self.threads,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Analyze {
            input,
            settings,
            report,
        } => {
            let result = analyze_png(input, &settings.config())?;
            emit_json(&result, report.as_ref())?;
        }
        Command::Correct {
            input,
            output,
            settings,
            overwrite,
            report,
        } => {
            let result = correct_png(input, output, &settings.config(), overwrite)?;
            emit_json(&result, report.as_ref())?;
        }
        Command::RawF32 { descriptor } => {
            let reports = correct_raw_f32(descriptor)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&reports)
                    .context("could not serialize float32 reports")?
            );
        }
    }
    Ok(())
}

fn emit_json(report: &CorrectionReport, destination: Option<&PathBuf>) -> Result<()> {
    let json = serde_json::to_string_pretty(report).context("could not serialize report")?;
    if let Some(path) = destination {
        ensure!(!path.as_os_str().is_empty(), "report path cannot be empty");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("could not create report directory: {}", parent.display())
            })?;
        }
        fs::write(path, json)
            .with_context(|| format!("could not write report: {}", path.display()))?;
    } else {
        println!("{json}");
    }
    Ok(())
}

fn parse_grid(value: &str) -> std::result::Result<GridSpec, String> {
    let normalized = value.trim().to_ascii_lowercase();
    let (columns, rows) = normalized
        .split_once('x')
        .ok_or_else(|| "grid must use COLSxROWS syntax, for example 2x2".to_owned())?;
    let columns = columns
        .parse::<u32>()
        .map_err(|_| "grid column count is not an unsigned integer".to_owned())?;
    let rows = rows
        .parse::<u32>()
        .map_err(|_| "grid row count is not an unsigned integer".to_owned())?;
    if columns == 0 || rows == 0 {
        return Err("grid dimensions must be non-zero".to_owned());
    }
    Ok(GridSpec { columns, rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_grid_shorthand() {
        assert_eq!(
            parse_grid("5x3").unwrap(),
            GridSpec {
                columns: 5,
                rows: 3
            }
        );
        assert!(parse_grid("5").is_err());
        assert!(parse_grid("0x2").is_err());
    }
}
