//! The audio path must not allocate, including while swapping processors.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use stepwave_core::stft::SAMPLE_RATE;
use stepwave_core::{Processor, Profile};
use stepwave_daemon::audio::{channel, WARMUP, XFADE};

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

#[test]
fn audio_core_swaps_without_allocating() {
    let profile = Profile::from_json(include_str!("../../profiles/cs2.json")).unwrap();
    let (mut handoff, mut core) = channel(Processor::new(&profile, SAMPLE_RATE).unwrap());
    let mut block = vec![0.1f32; 256 * 2];

    // Warm up (first calls may touch lazily initialised state in dependencies).
    for _ in 0..10 {
        core.process_interleaved(&mut block);
    }

    // Built and published on "the control thread" — allocation allowed here.
    assert!(handoff
        .publish(Box::new(Processor::new(&profile, SAMPLE_RATE).unwrap()))
        .is_ok());

    let before = allocs();
    for _ in 0..((WARMUP + XFADE) / 256 + 20) {
        core.process_interleaved(&mut block);
    }
    assert_eq!(allocs() - before, 0, "audio path allocated during a swap");
    assert_eq!(handoff.collect(), 1);
}
