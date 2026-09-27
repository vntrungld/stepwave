//! The audio path must not allocate: count allocations made on this thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use stepwave_core::erb::NUM_BANDS;
use stepwave_core::mask::{MaskSource, ModelMask};
use stepwave_core::stft::{Complex32, BINS, HOP};
use stepwave_core::swm::SwmModel;
use stepwave_core::testing::white_noise;
use stepwave_core::{Processor, Profile};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn allocs() -> usize {
    ALLOCS.with(|c| c.get())
}

const FIXTURE: &[u8] = include_bytes!("fixtures/model_random.swm");
const CS2: &str = include_str!("../../profiles/cs2.json");

#[test]
fn model_mask_and_processor_do_not_allocate() {
    let model = SwmModel::from_bytes(FIXTURE).unwrap();
    let mut mask = ModelMask::new(&model, 6.0);
    let spectrum: Vec<Complex32> = white_noise(1, BINS, 1.0)
        .into_iter()
        .map(|v| Complex32::new(v, -v))
        .collect();
    let mut gains = [0.0; NUM_BANDS];
    let mut p = Processor::with_model(&Profile::from_json(CS2).unwrap(), &model, 48_000).unwrap();
    let x = white_noise(2, HOP, 0.3);
    let (mut l, mut r) = (x.clone(), x.clone());

    let before = allocs();
    for _ in 0..100 {
        mask.next_mask(&spectrum, &mut gains);
    }
    mask.reset();
    for _ in 0..100 {
        l.copy_from_slice(&x);
        r.copy_from_slice(&x);
        p.process(&mut l, &mut r);
    }
    p.reset();
    assert_eq!(allocs() - before, 0, "allocations in the audio path");
}
