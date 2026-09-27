use serde::Deserialize;
use stepwave_core::erb::NUM_BANDS;
use stepwave_core::gru::ModelRunner;
use stepwave_core::swm::SwmModel;

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
