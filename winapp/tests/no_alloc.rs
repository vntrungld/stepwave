//! The capture push and render pull (Slip resampler + controller) must not allocate or free.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use stepwave_winapp::drift::{channel, CHANNELS, RESYNC_FACTOR};

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

#[test]
fn discard_and_reset_do_not_allocate() {
    let (mut cap, mut ren, stats) = channel();
    cap.set_period(480);
    ren.set_period(480);
    let packet = vec![0.3f32; 480 * CHANNELS];
    let mut out = vec![0.0f32; 480 * CHANNELS];

    // Warm up: first calls may touch lazily initialised state.
    for _ in 0..4 {
        cap.push(&packet);
        ren.render(&mut out);
    }
    let target = stats.target_frames() as usize;
    let mut first = vec![0.0f32; 1056 * CHANNELS];

    let before = counts();

    // Overflow-resync path: push well past `RESYNC_FACTOR * target` without rendering, so the
    // next render()'s first chunk finds the ring overfull and discards the excess through the
    // `read_chunk`/`commit_all` path in `discard()`.
    let overflow_packets = (RESYNC_FACTOR * target) / 480 + 4;
    for _ in 0..overflow_packets {
        cap.push(&packet);
    }
    ren.render(&mut out);
    assert!(stats.resyncs() > 0, "overflow-resync path not exercised");

    // reset(): declared periods survive it, so this also exercises `discard()` again if the
    // ring is still above the (unchanged) target.
    ren.reset();
    ren.render(&mut out);

    // Priming trim: a device reopen with capture still running (70 ms of packets), then
    // WASAPI's whole-endpoint-buffer first request.
    ren.reset();
    for _ in 0..7 {
        cap.push(&packet);
    }
    ren.render(&mut first);

    let after = counts();
    assert_eq!(after.0 - before.0, 0, "discard/reset path allocated");
    assert_eq!(after.1 - before.1, 0, "discard/reset path freed memory");
}
