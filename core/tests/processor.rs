use approx::assert_abs_diff_eq;
use stepwave_core::erb::NUM_BANDS;
use stepwave_core::limiter::CEILING;
use stepwave_core::mask::{eq_coefficients, magnitude_db, MaskSource, UnityMask};
use stepwave_core::stft::{Complex32, WIN};
use stepwave_core::testing::{db, rms, sine, white_noise};
use stepwave_core::{CoreError, Processor, Profile};

const CS2: &str = include_str!("../../profiles/cs2.json");

fn cs2() -> Processor {
    Processor::new(&Profile::from_json(CS2).unwrap(), 48_000).unwrap()
}

fn unity() -> Processor {
    Processor::with_mask(Box::new(UnityMask), 48_000).unwrap()
}

fn run(p: &mut Processor, l: &[f32], r: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (l.to_vec(), r.to_vec());
    for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        p.process(a, b);
    }
    (l, r)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn reports_one_window_of_latency() {
    assert_eq!(unity().latency_samples(), WIN);
}

#[test]
fn rejects_other_sample_rates() {
    let p = Profile::from_json(CS2).unwrap();
    assert!(matches!(
        Processor::new(&p, 44_100),
        Err(CoreError::UnsupportedSampleRate(44_100))
    ));
}

#[test]
fn unity_mask_reconstructs_input_delayed_by_latency() {
    let x = white_noise(7, 48_000, 0.25);
    let (l, _) = run(&mut unity(), &x, &x, 480);
    let err: Vec<f32> = l[WIN..].iter().zip(&x).map(|(y, x)| y - x).collect();
    let ratio = db(rms(&err) / rms(&x[..x.len() - WIN]));
    assert!(ratio < -90.0, "reconstruction error {ratio} dB");
}

#[test]
fn cs2_static_eq_matches_design_within_tolerance() {
    let profile = Profile::from_json(CS2).unwrap();
    for freq in [120.0f32, 2500.0, 5000.0] {
        let x = sine(freq, 0.1, 48_000);
        let (l, _) = run(&mut cs2(), &x, &x, 480);
        let measured = db(rms(&l[24_000..]) / rms(&x[24_000..]));
        let expected = profile.preamp_db
            + profile
                .fallback_eq
                .iter()
                .map(|b| magnitude_db(&eq_coefficients(b).unwrap(), freq))
                .sum::<f32>();
        assert!(
            (measured - expected).abs() <= 1.5,
            "{freq} Hz: measured {measured:.2} dB, expected {expected:.2} dB"
        );
    }
}

#[test]
fn stereo_image_is_preserved() {
    let n = white_noise(3, 48_000, 0.3);
    let l_in: Vec<f32> = n.iter().map(|v| v * 0.7).collect();
    let r_in: Vec<f32> = n.iter().map(|v| v * 0.3).collect();
    let (l, r) = run(&mut cs2(), &l_in, &r_in, 480);
    let (l, r) = (&l[WIN..], &r[WIN..]);
    assert_abs_diff_eq!(
        db(rms(l) / rms(r)),
        db(rms(&l_in) / rms(&r_in)),
        epsilon = 0.1
    );
    let dot: f32 = l.iter().zip(r).map(|(a, b)| a * b).sum();
    let corr =
        dot / (l.iter().map(|a| a * a).sum::<f32>() * r.iter().map(|b| b * b).sum::<f32>()).sqrt();
    assert!(corr >= 0.99, "L/R correlation {corr}");
}

#[test]
fn loud_input_never_exceeds_ceiling() {
    let x = sine(1000.0, 3.98, 48_000); // +12 dBFS
    for mut p in [cs2(), unity()] {
        let (l, r) = run(&mut p, &x, &x, 480);
        assert!(peak(&l) <= CEILING && peak(&r) <= CEILING);
    }
}

#[test]
fn output_is_independent_of_block_size() {
    let x = white_noise(11, 12_000, 0.3);
    let y = white_noise(12, 12_000, 0.3);
    let reference = run(&mut cs2(), &x, &y, 480);
    for block in [1, 64, 1000] {
        assert_eq!(run(&mut cs2(), &x, &y, block), reference, "block {block}");
    }
}

#[test]
fn silence_stays_silent() {
    let x = vec![0.0; 9_600];
    let (l, r) = run(&mut cs2(), &x, &x, 480);
    assert!(l.iter().chain(&r).all(|v| *v == 0.0));
}

struct NanMask;

impl MaskSource for NanMask {
    fn next_mask(&mut self, _: &[Complex32], gains_db: &mut [f32; NUM_BANDS]) {
        gains_db.fill(f32::NAN);
    }
}

#[test]
fn non_finite_mask_falls_back_to_unity() {
    let x = white_noise(5, 9_600, 0.3);
    let mut nan = Processor::with_mask(Box::new(NanMask), 48_000).unwrap();
    let out = run(&mut nan, &x, &x, 480);
    assert!(out.0.iter().all(|v| v.is_finite()));
    assert_eq!(out, run(&mut unity(), &x, &x, 480));
}

struct HugeMask;

impl MaskSource for HugeMask {
    fn next_mask(&mut self, _: &[Complex32], gains_db: &mut [f32; NUM_BANDS]) {
        gains_db.fill(1.0e4);
    }
}

#[test]
fn huge_finite_mask_is_clamped_and_stays_finite() {
    let x = white_noise(9, 9_600, 0.3);
    let mut huge = Processor::with_mask(Box::new(HugeMask), 48_000).unwrap();
    let (l, r) = run(&mut huge, &x, &x, 480);
    assert!(
        l.iter().chain(&r).all(|v| v.is_finite()),
        "huge mask must not overflow to inf/NaN"
    );
    assert!(peak(&l) <= CEILING && peak(&r) <= CEILING);
}

#[test]
fn nan_input_sample_stays_finite_and_output_resyncs_with_clean_run() {
    let len = 9_600;
    let clean = white_noise(13, len, 0.3);
    let mut dirty = clean.clone();
    dirty[100] = f32::NAN;

    let (out_clean, _) = run(&mut unity(), &clean, &clean, 480);
    let (out_dirty, _) = run(&mut unity(), &dirty, &dirty, 480);

    assert!(
        out_dirty.iter().all(|v| v.is_finite()),
        "a single bad input sample must never leave non-finite output"
    );

    // A single NaN sample poisons every FFT bin of any analysis frame whose
    // 960-sample (WIN) window still contains it -- at most 2 consecutive hops
    // (WIN = 2*HOP) -- and, because the overlap-add accumulator is zero-filled
    // each time a hop is read out, one further hop is needed for a subsequent
    // clean frame's contribution to land on a fully-reset slot and resynchronise
    // bit-for-bit with a run that never saw the bad sample. So at most ~3 hops
    // (1440 samples) around the injection point can differ; we compare the last
    // 4_800 samples (10 hops), a comfortable margin past that settling window.
    let tail = len - 4_800;
    for (i, (d, c)) in out_dirty[tail..].iter().zip(&out_clean[tail..]).enumerate() {
        assert!(
            (d - c).abs() < 1e-6,
            "sample {} diverged from clean run: dirty={d} clean={c}",
            tail + i
        );
    }
}
