use serde::Deserialize;
use stepwave_core::erb::NUM_BANDS;
use stepwave_core::gru::ModelRunner;
use stepwave_core::limiter::CEILING;
use stepwave_core::mask::{MaskSource, ModelMask};
use stepwave_core::stft::{Complex32, BINS};
use stepwave_core::swm::SwmModel;
use stepwave_core::testing::{sine, white_noise};
use stepwave_core::{Processor, Profile};

pub const FIXTURE: &[u8] = include_bytes!("fixtures/model_random.swm");
const EXPECTED: &str = include_str!("fixtures/model_random_expected.json");

#[derive(Deserialize)]
struct Expected {
    frames: Vec<[f32; NUM_BANDS]>,
    gains: Vec<[f64; NUM_BANDS]>,
}

fn model() -> SwmModel {
    SwmModel::from_bytes(FIXTURE).unwrap()
}

fn expected() -> Expected {
    serde_json::from_str(EXPECTED).unwrap()
}

fn run(runner: &mut ModelRunner, frames: &[[f32; NUM_BANDS]]) -> Vec<[f32; NUM_BANDS]> {
    frames
        .iter()
        .map(|e| {
            let mut g = [0.0; NUM_BANDS];
            runner.step(e, &mut g);
            g
        })
        .collect()
}

#[test]
fn matches_numpy_reference() {
    let exp = expected();
    assert_eq!(exp.frames.len(), 200);
    let got = run(&mut ModelRunner::new(&model()), &exp.frames);
    let mut worst = 0.0f64;
    for (g, e) in got.iter().zip(&exp.gains) {
        for (a, b) in g.iter().zip(e) {
            worst = worst.max((*a as f64 - b).abs());
        }
    }
    assert!(worst <= 1e-3, "max abs error {worst} dB");
}

#[test]
fn reset_restarts_the_stream() {
    let exp = expected();
    let mut runner = ModelRunner::new(&model());
    let first = run(&mut runner, &exp.frames);
    runner.reset();
    assert_eq!(run(&mut runner, &exp.frames), first);
}

#[test]
fn silence_gives_finite_gains() {
    let mut runner = ModelRunner::new(&model());
    for level in [-100.0f32, 0.0, -100.0, 40.0] {
        let mut g = [0.0; NUM_BANDS];
        runner.step(&[level; NUM_BANDS], &mut g);
        assert!(
            g.iter()
                .all(|v| v.is_finite() && (-12.0..=12.0).contains(v)),
            "{g:?}"
        );
    }
}

const CS2: &str = include_str!("../../profiles/cs2.json");

fn spectrum(seed: u64) -> Vec<Complex32> {
    let re = white_noise(seed, BINS, 1.0);
    let im = white_noise(seed + 1, BINS, 1.0);
    re.into_iter()
        .zip(im)
        .map(|(a, b)| Complex32::new(a, b))
        .collect()
}

fn model_processor() -> Processor {
    Processor::with_model(&Profile::from_json(CS2).unwrap(), &model(), 48_000).unwrap()
}

fn process(p: &mut Processor, l: &[f32], r: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (l.to_vec(), r.to_vec());
    for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        p.process(a, b);
    }
    (l, r)
}

#[test]
fn strength_scales_gains_linearly() {
    let m = model();
    let mut full = ModelMask::new(&m, 6.0);
    let mut half = ModelMask::new(&m, 3.0);
    let mut off = ModelMask::new(&m, 0.0);
    for seed in 0..20 {
        let s = spectrum(seed * 2);
        let (mut a, mut b, mut c) = ([0.0; NUM_BANDS], [0.0; NUM_BANDS], [0.0; NUM_BANDS]);
        full.next_mask(&s, &mut a);
        half.next_mask(&s, &mut b);
        off.next_mask(&s, &mut c);
        for i in 0..NUM_BANDS {
            assert!(
                (a[i] * 0.5 - b[i]).abs() < 1e-5,
                "band {i}: {} vs {}",
                a[i],
                b[i]
            );
            assert_eq!(c[i], 0.0);
        }
    }
}

#[test]
fn mask_reset_matches_fresh_mask() {
    let m = model();
    let mut used = ModelMask::new(&m, 6.0);
    let mut g = [0.0; NUM_BANDS];
    for seed in 0..10 {
        used.next_mask(&spectrum(seed), &mut g);
    }
    used.reset();
    let mut fresh = ModelMask::new(&m, 6.0);
    for seed in 50..60 {
        let (mut a, mut b) = ([0.0; NUM_BANDS], [0.0; NUM_BANDS]);
        used.next_mask(&spectrum(seed), &mut a);
        fresh.next_mask(&spectrum(seed), &mut b);
        assert_eq!(a, b);
    }
}

#[test]
fn model_processor_is_block_size_independent() {
    let x = white_noise(11, 48_000, 0.2);
    let y = white_noise(12, 48_000, 0.2);
    let reference = process(&mut model_processor(), &x, &y, 480);
    for block in [1, 64, 1000] {
        assert_eq!(
            process(&mut model_processor(), &x, &y, block),
            reference,
            "block {block}"
        );
    }
}

#[test]
fn model_processor_never_clips() {
    let x = sine(1000.0, 4.0, 48_000); // +12 dBFS
    let (l, r) = process(&mut model_processor(), &x, &x, 480);
    let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak <= CEILING, "peak {peak}");
}

#[test]
fn model_processor_preserves_stereo_image() {
    let n = white_noise(13, 96_000, 0.3);
    let l_in: Vec<f32> = n.iter().map(|v| v * 0.7).collect();
    let r_in: Vec<f32> = n.iter().map(|v| v * 0.3).collect();
    let (l, r) = process(&mut model_processor(), &l_in, &r_in, 480);
    let (l, r) = (&l[4800..], &r[4800..]);
    let energy = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    let ratio_in = 10.0 * (0.7f64 * 0.7 / (0.3 * 0.3)).log10();
    let ratio_out = 10.0 * (energy(l) / energy(r)).log10();
    assert!(
        (ratio_out - ratio_in).abs() < 0.1,
        "{ratio_out} vs {ratio_in}"
    );
    let dot: f64 = l.iter().zip(r).map(|(a, b)| *a as f64 * *b as f64).sum();
    let corr = dot / (energy(l) * energy(r)).sqrt();
    assert!(corr > 0.99, "correlation {corr}");
}

#[test]
fn model_mask_recovers_from_nan_and_inf_band_energy() {
    // Two masks kept in lockstep on identical clean frames; `used` additionally
    // sees two frames whose mid spectrum has one non-finite bin each (a NaN and
    // an +inf), `reference` sees the same frames unmodified. A single glitched
    // bin drives 2 of the 32 band energies non-finite (erb::band_energies_db
    // splits each FFT bin's power between its two neighbouring bands).
    let m = model();
    let mut used = ModelMask::new(&m, 6.0);
    let mut reference = ModelMask::new(&m, 6.0);
    let mut g = [0.0; NUM_BANDS];

    for seed in 0..5 {
        let s = spectrum(seed);
        used.next_mask(&s, &mut g);
        reference.next_mask(&s, &mut g);
    }

    let clean_50 = spectrum(50);
    let mut nan_50 = clean_50.clone();
    nan_50[10] = Complex32::new(f32::NAN, 0.0);
    used.next_mask(&nan_50, &mut g);
    assert!(
        g.iter().all(|v| v.is_finite()),
        "a NaN band energy must never leave a non-finite gain: {g:?}"
    );
    reference.next_mask(&clean_50, &mut g);

    let clean_51 = spectrum(51);
    let mut inf_51 = clean_51.clone();
    inf_51[10] = Complex32::new(f32::INFINITY, 0.0);
    used.next_mask(&inf_51, &mut g);
    assert!(
        g.iter().all(|v| v.is_finite()),
        "a +inf band energy must never leave a non-finite gain: {g:?}"
    );
    reference.next_mask(&clean_51, &mut g);

    // Let ~1 s (100 frames) of identical clean input pass, for the recurrent
    // state's fading memory to settle, before comparing.
    for seed in 100..200 {
        let s = spectrum(seed);
        used.next_mask(&s, &mut g);
        reference.next_mask(&s, &mut g);
    }

    // The two masks must now agree closely: the model's recurrent state must
    // not stay permanently corrupted by the earlier non-finite frames.
    for seed in 200..300 {
        let s = spectrum(seed);
        let (mut a, mut b) = ([0.0; NUM_BANDS], [0.0; NUM_BANDS]);
        used.next_mask(&s, &mut a);
        reference.next_mask(&s, &mut b);
        for i in 0..NUM_BANDS {
            assert!(
                (a[i] - b[i]).abs() < 1e-3,
                "seed {seed} band {i}: used {} vs reference {}",
                a[i],
                b[i]
            );
        }
    }
}

#[test]
fn processor_reset_equals_fresh_processor() {
    let x = white_noise(14, 24_000, 0.3);
    let mut used = model_processor();
    process(&mut used, &x, &x, 480);
    used.reset();
    let y = white_noise(15, 24_000, 0.3);
    assert_eq!(
        process(&mut used, &y, &y, 480),
        process(&mut model_processor(), &y, &y, 480)
    );
}
