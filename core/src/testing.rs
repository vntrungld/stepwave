//! Deterministic synthetic signals for tests. Not used in the audio path.

use std::f64::consts::TAU;

/// Uniform white noise in `[-amplitude, amplitude)` from a fixed-seed LCG.
pub fn white_noise(seed: u64, len: usize, amplitude: f32) -> Vec<f32> {
    const A: u64 = 6_364_136_223_846_793_005;
    const C: u64 = 1_442_695_040_888_963_407;
    let mut state = seed.wrapping_mul(A).wrapping_add(C);
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(A).wrapping_add(C);
            let unit = (state >> 40) as f32 / (1u64 << 24) as f32;
            (unit * 2.0 - 1.0) * amplitude
        })
        .collect()
}

/// Sine at `freq_hz`, sampled at 48 kHz.
pub fn sine(freq_hz: f32, amplitude: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| amplitude * (TAU * freq_hz as f64 * n as f64 / 48_000.0).sin() as f32)
        .collect()
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Amplitude ratio to decibels.
pub fn db(ratio: f32) -> f32 {
    20.0 * ratio.log10()
}
