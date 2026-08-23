use crate::color::{Rgb, norm, scale, sub};

#[derive(Clone, Copy, Debug)]
pub struct RobustEstimate {
    pub center: Rgb,
    pub dispersion: f64,
    pub effective_weight: f64,
}

#[must_use]
pub fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
}

#[must_use]
pub fn robust_rgb(samples: &[(Rgb, f64)]) -> Option<RobustEstimate> {
    if samples.len() < 4 {
        return None;
    }

    let mut channels = [Vec::new(), Vec::new(), Vec::new()];
    for (value, weight) in samples {
        if *weight > 0.0 && value.iter().all(|channel| channel.is_finite()) {
            for channel in 0..3 {
                channels[channel].push(value[channel]);
            }
        }
    }
    if channels[0].len() < 4 {
        return None;
    }

    let mut center = [
        median(&mut channels[0]),
        median(&mut channels[1]),
        median(&mut channels[2]),
    ];
    let mut residuals: Vec<f64> = samples
        .iter()
        .map(|(value, _)| norm(sub(*value, center)))
        .collect();
    let mut dispersion = (1.4826 * median(&mut residuals)).max(1.0e-6);

    for _ in 0..6 {
        let cutoff = 1.5 * dispersion;
        let mut sum = [0.0; 3];
        let mut total = 0.0;
        for (value, base_weight) in samples {
            let residual = norm(sub(*value, center));
            let huber = if residual <= cutoff {
                1.0
            } else {
                cutoff / residual.max(1.0e-12)
            };
            let weight = base_weight.max(0.0) * huber;
            for channel in 0..3 {
                sum[channel] += value[channel] * weight;
            }
            total += weight;
        }
        if total <= 0.0 {
            return None;
        }
        center = scale(sum, 1.0 / total);
        residuals.clear();
        residuals.extend(samples.iter().map(|(value, _)| norm(sub(*value, center))));
        dispersion = (1.4826 * median(&mut residuals)).max(1.0e-6);
    }

    let effective_weight = samples.iter().map(|(_, weight)| weight.max(0.0)).sum();
    Some(RobustEstimate {
        center,
        dispersion,
        effective_weight,
    })
}

#[must_use]
pub fn smooth_profile(profile: &[Rgb], radius: usize) -> Vec<Rgb> {
    if radius == 0 || profile.len() < 3 {
        return profile.to_vec();
    }
    let mut current = profile.to_vec();
    for _ in 0..3 {
        current = box_smooth(&current, radius);
    }
    current
}

fn box_smooth(profile: &[Rgb], radius: usize) -> Vec<Rgb> {
    let mut prefix = vec![[0.0; 3]; profile.len() + 1];
    for (index, value) in profile.iter().enumerate() {
        prefix[index + 1] = [
            prefix[index][0] + value[0],
            prefix[index][1] + value[1],
            prefix[index][2] + value[2],
        ];
    }
    (0..profile.len())
        .map(|index| {
            let start = index.saturating_sub(radius);
            let end = (index + radius + 1).min(profile.len());
            let count = (end - start) as f64;
            [
                (prefix[end][0] - prefix[start][0]) / count,
                (prefix[end][1] - prefix[start][1]) / count,
                (prefix[end][2] - prefix[start][2]) / count,
            ]
        })
        .collect()
}
