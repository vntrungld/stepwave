//! The audio path must not allocate, including while swapping processors. It must also
//! not *deallocate*: a drop on the audio thread (e.g. a retired processor silently
//! overwritten instead of handed back) frees memory just as surely as a bug that
//! allocates, so both are counted and asserted zero over the swap window.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use stepwave_core::stft::SAMPLE_RATE;
use stepwave_core::{Processor, Profile};
use stepwave_host::audio::{channel, WARMUP, XFADE};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = DEALLOCS.try_with(|c| c.set(c.get() + 1));
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

fn deallocs() -> usize {
    DEALLOCS.with(|c| c.get())
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

    let allocs_before = allocs();
    let deallocs_before = deallocs();
    for _ in 0..((WARMUP + XFADE) / 256 + 20) {
        core.process_interleaved(&mut block);
    }
    assert_eq!(
        allocs() - allocs_before,
        0,
        "audio path allocated during a swap"
    );
    assert_eq!(
        deallocs() - deallocs_before,
        0,
        "audio path deallocated (e.g. dropped a processor) during a swap"
    );
    // Collecting the retiree is control-thread work and happens after the measured
    // window, so its drop (on this thread, not the audio thread) is not counted above.
    assert_eq!(handoff.collect(), 1);
}
