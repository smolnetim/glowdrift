//! Micro-benchmark: prints nanoseconds per `reduce` call on a 128x72 frame.
//!
//! Run with `cargo run --release -p glowdrift-core --example bench_reduce`.

use std::hint::black_box;
use std::time::Instant;

use glowdrift_core::{FrameView, ReduceMode, reduce};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 72;
const ITERATIONS: u32 = 2000;

fn synthetic_frame() -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let noise = (state >> 56) as u8 / 8;
            data.extend_from_slice(&[
                (x * 2) as u8 ^ noise,
                (y * 3) as u8 ^ noise,
                ((x + y) * 2) as u8 ^ noise,
                255,
            ]);
        }
    }
    data
}

fn main() {
    let data = synthetic_frame();
    let frame = FrameView::new(&data, WIDTH, HEIGHT, WIDTH as usize * 4);
    for mode in [ReduceMode::Average, ReduceMode::Dominant] {
        black_box(reduce(black_box(&frame), mode));
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            black_box(reduce(black_box(&frame), mode));
        }
        let ns = start.elapsed().as_nanos() / u128::from(ITERATIONS);
        println!("{mode:?}: {ns} ns/frame at {WIDTH}x{HEIGHT}");
    }
}
