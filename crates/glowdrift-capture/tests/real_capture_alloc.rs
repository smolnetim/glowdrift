//! Steady-state grabs must not allocate on the heap. Needs a real, interactive Windows desktop;
//! run with `--ignored`. It has its own binary because it installs a global allocator.

#![cfg(windows)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Duration;

use glowdrift_capture::Capturer;
use glowdrift_core::NormRect;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// Counts allocations made by the calling thread only, so other test threads do not interfere.
struct Counting;

fn count() {
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
}

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

const WARMUP: usize = 6;
const MEASURED: usize = 40;

#[test]
#[ignore = "needs an interactive desktop"]
fn steady_state_grabs_do_not_allocate() {
    let mut capturer = Capturer::new(None, 128).expect("capturer");
    // Two regions of equal size: every grab renders and reads back a fresh frame even when the
    // screen is static.
    let regions = [
        NormRect::new(0.0, 0.0, 0.5, 0.5),
        NormRect::new(0.5, 0.5, 0.5, 0.5),
    ];
    let timeout = Duration::from_secs(2);
    for i in 0..WARMUP {
        capturer
            .grab(regions[i % 2], timeout)
            .expect("grab")
            .expect("frame");
    }

    let before = allocations();
    for i in 0..MEASURED {
        let frame = capturer
            .grab(regions[i % 2], timeout)
            .expect("grab")
            .expect("a region change delivers a frame");
        assert!(frame.is_valid());
    }
    let used = allocations() - before;
    assert_eq!(used, 0, "{used} heap allocations over {MEASURED} grabs");
}

#[test]
#[ignore = "needs an interactive desktop"]
fn steady_state_grabs_of_one_region_do_not_allocate() {
    let mut capturer = Capturer::new(None, 128).expect("capturer");
    let region = NormRect::full();
    let timeout = Duration::from_millis(50);
    for _ in 0..WARMUP {
        capturer.grab(region, timeout).expect("grab");
    }

    // On a static screen these are timeouts; on a changing one they deliver frames through the
    // asynchronous read-back.
    let before = allocations();
    let mut delivered = 0;
    for _ in 0..MEASURED {
        if capturer.grab(region, timeout).expect("grab").is_some() {
            delivered += 1;
        }
    }
    let used = allocations() - before;
    assert_eq!(
        used, 0,
        "{used} heap allocations over {MEASURED} grabs ({delivered} delivered)"
    );
}
