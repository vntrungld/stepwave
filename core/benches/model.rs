//! Cost of the model per 10 ms frame (budget: < 1 ms). Run: cargo bench -p stepwave-core

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use stepwave_core::erb::NUM_BANDS;
use stepwave_core::mask::{MaskSource, ModelMask};
use stepwave_core::stft::{Complex32, BINS, HOP};
use stepwave_core::swm::SwmModel;
use stepwave_core::testing::white_noise;
use stepwave_core::{Processor, Profile};

const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/model_random.swm");
const CS2: &str = include_str!("../../profiles/cs2.json");

fn bench(c: &mut Criterion) {
    let model = SwmModel::from_bytes(FIXTURE).unwrap();
    let profile = Profile::from_json(CS2).unwrap();

    let spectrum: Vec<Complex32> = white_noise(1, BINS, 1.0)
        .into_iter()
        .map(|v| Complex32::new(v, 0.5 * v))
        .collect();
    let mut mask = ModelMask::new(&model, profile.strength_db);
    let mut gains = [0.0; NUM_BANDS];
    c.bench_function("model_mask_next_mask", |b| {
        b.iter(|| mask.next_mask(black_box(&spectrum), &mut gains))
    });

    let x = white_noise(2, HOP, 0.3);
    let processors = [
        (
            "process_hop_model",
            Processor::with_model(&profile, &model, 48_000).unwrap(),
        ),
        ("process_hop_eq", Processor::new(&profile, 48_000).unwrap()),
    ];
    for (name, mut p) in processors {
        let (mut l, mut r) = (x.clone(), x.clone());
        c.bench_function(name, |b| {
            b.iter(|| {
                l.copy_from_slice(&x);
                r.copy_from_slice(&x);
                p.process(black_box(&mut l), black_box(&mut r));
            })
        });
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
