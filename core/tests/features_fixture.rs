//! Pins the feature values that Python training code must reproduce via `stepwave_py`.
//! Regenerate with `STEPWAVE_REGEN_FIXTURES=1 cargo test -p stepwave-core --test features_fixture`.

use stepwave_core::erb::NUM_BANDS;
use stepwave_core::features::FeatureExtractor;
use stepwave_core::testing::sine;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/features_sine_1k.json"
);

fn compute() -> Vec<[f32; NUM_BANDS]> {
    let x = sine(1000.0, 0.5, 4800);
    FeatureExtractor::new().band_energies_db(&x, &x)
}

#[test]
fn matches_committed_fixture() {
    let rows = compute();
    if std::env::var_os("STEPWAVE_REGEN_FIXTURES").is_some() {
        let json = serde_json::json!({
            "signal": {"kind": "sine", "freq_hz": 1000.0, "amplitude": 0.5, "len": 4800},
            "frames": rows,
        });
        std::fs::write(FIXTURE, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }
    let text = std::fs::read_to_string(FIXTURE).expect("fixture missing: regenerate it");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let frames: Vec<Vec<f32>> = serde_json::from_value(json["frames"].clone()).unwrap();
    assert_eq!(frames.len(), rows.len());
    for (a, b) in frames.iter().flatten().zip(rows.iter().flatten()) {
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }
}
