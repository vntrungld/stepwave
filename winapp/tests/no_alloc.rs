//! The capture push and render pull (Slip resampler + controller) must not allocate or free.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use stepwave_winapp::drift::{channel, CHANNELS};

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

fn counts() -> (usize, usize) {
    (ALLOCS.with(|c| c.get()), DEALLOCS.with(|c| c.get()))
}

#[test]
fn push_and_render_do_not_allocate() {
    let (mut cap, mut ren, stats) = channel();
    let packet = vec![0.3f32; 480 * CHANNELS];
    let mut out = vec![0.0f32; 441 * CHANNELS];

    // Warm up: first calls may touch lazily initialised state.
    for _ in 0..4 {
        cap.push(&packet);
        ren.render(&mut out);
    }

    let before = counts();
    // Uneven periods (480 in, 441 out) force priming, controller updates and frame slips.
    for i in 0..3000 {
        cap.push(&packet);
        ren.render(&mut out);
        if i % 11 == 0 {
            ren.render(&mut out);
        }
    }
    let after = counts();
    assert_eq!(after.0 - before.0, 0, "render path allocated");
    assert_eq!(after.1 - before.1, 0, "render path freed memory");
    assert!(stats.fill_frames() > 0);
}
