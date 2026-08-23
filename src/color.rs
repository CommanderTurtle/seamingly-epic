use crate::config::TransferFunction;

pub type Rgb = [f64; 3];

#[must_use]
pub fn decode_sample(value: f64, transfer: TransferFunction) -> f64 {
    let value = value.clamp(0.0, 1.0);
    match transfer {
        TransferFunction::Linear => value,
        TransferFunction::Srgb if value <= 0.040_45 => value / 12.92,
        TransferFunction::Srgb => ((value + 0.055) / 1.055).powf(2.4),
    }
}

#[must_use]
pub fn encode_sample(value: f64, transfer: TransferFunction) -> f64 {
    let value = value.clamp(0.0, 1.0);
    match transfer {
        TransferFunction::Linear => value,
        TransferFunction::Srgb if value <= 0.003_130_8 => 12.92 * value,
        TransferFunction::Srgb => 1.055 * value.powf(1.0 / 2.4) - 0.055,
    }
}

#[must_use]
pub fn log_rgb(rgb: Rgb) -> Option<Rgb> {
    const MIN: f64 = 1.0 / 65_535.0;
    const MAX: f64 = 0.995;
    if rgb
        .iter()
        .any(|value| !value.is_finite() || *value <= MIN || *value >= MAX)
    {
        return None;
    }
    Some(rgb.map(f64::ln))
}

#[must_use]
pub fn add(a: Rgb, b: Rgb) -> Rgb {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[must_use]
pub fn sub(a: Rgb, b: Rgb) -> Rgb {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[must_use]
pub fn scale(value: Rgb, factor: f64) -> Rgb {
    [value[0] * factor, value[1] * factor, value[2] * factor]
}

#[must_use]
pub fn norm(value: Rgb) -> f64 {
    (value[0].mul_add(value[0], value[1].mul_add(value[1], value[2] * value[2])) / 3.0).sqrt()
}

#[must_use]
pub fn apply_log_gain(rgb: Rgb, gain: Rgb) -> Rgb {
    std::array::from_fn(|channel| {
        if rgb[channel] >= 1.0 {
            // A clipped source channel contains no recoverable photometric
            // magnitude. Keep its exact endpoint instead of inventing gray
            // from a neighboring seam estimate.
            1.0
        } else {
            (rgb[channel] * gain[channel].exp()).clamp(0.0, 1.0)
        }
    })
}

#[must_use]
pub fn log_gain_to_stops(gain: Rgb) -> Rgb {
    let divisor = std::f64::consts::LN_2;
    gain.map(|value| value / divisor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_white_channels_remain_exact_white() {
        assert_eq!(
            apply_log_gain([1.0, 1.0, 1.0], [-2.0, -0.5, 1.0]),
            [1.0, 1.0, 1.0]
        );
        assert_eq!(apply_log_gain([1.0, 0.5, 0.25], [-2.0, 0.0, 0.0])[0], 1.0);
    }
}
