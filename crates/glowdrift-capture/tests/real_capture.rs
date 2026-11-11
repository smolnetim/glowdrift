//! Integration tests that need a real, interactive Windows desktop. Run with `--ignored`.

#![cfg(windows)]

use std::time::Duration;

use glowdrift_capture::{Capturer, capture_thumbnail, list_monitors};
use glowdrift_core::NormRect;

fn mean_bgr(data: &[u8]) -> [f64; 3] {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for px in data.as_chunks::<4>().0 {
        sum[0] += u64::from(px[0]);
        sum[1] += u64::from(px[1]);
        sum[2] += u64::from(px[2]);
        n += 1;
    }
    sum.map(|s| s as f64 / n.max(1) as f64)
}

#[test]
#[ignore = "needs an interactive desktop"]
fn lists_at_least_one_monitor_with_stable_ids() {
    let first = list_monitors().expect("enumerate");
    assert_ne!(first.len(), 0);
    assert_eq!(first.iter().filter(|m| m.primary).count(), 1);
    assert!(first[0].primary);
    let second = list_monitors().expect("enumerate again");
    let ids =
        |v: &[glowdrift_capture::MonitorInfo]| v.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&first), ids(&second));
    let mut unique = ids(&first);
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), first.len());
}

#[test]
#[ignore = "needs an interactive desktop"]
fn captures_a_real_frame_and_scales_consistently() {
    let mut small = Capturer::new(None, 64).expect("capturer");
    let frame = small
        .grab(NormRect::full(), Duration::from_secs(2))
        .expect("grab")
        .expect("first grab delivers the current desktop image");
    assert!(frame.view().is_valid());
    assert!(frame.width.max(frame.height) >= 64);
    assert!(frame.width.max(frame.height) < 128 + 1);
    assert!(frame.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255));

    // Windows allows one duplication per output per process, so release the first session.
    drop(small);

    // A larger capture of the same screen has a similar mean colour (screens may change a bit).
    let mut big = Capturer::new(None, 1024).expect("capturer");
    let big_frame = big
        .grab(NormRect::full(), Duration::from_secs(2))
        .expect("grab")
        .expect("frame");
    assert!(big_frame.width.max(big_frame.height) >= 1024);
    let (a, b) = (mean_bgr(&frame.data), mean_bgr(&big_frame.data));
    for c in 0..3 {
        assert!(
            (a[c] - b[c]).abs() < 12.0,
            "channel {c}: {} vs {}",
            a[c],
            b[c]
        );
    }
}

#[test]
#[ignore = "needs an interactive desktop"]
fn region_change_on_static_screen_returns_immediately() {
    let mut capturer = Capturer::new(None, 32).expect("capturer");
    let full = capturer
        .grab(NormRect::full(), Duration::from_secs(2))
        .expect("grab");
    assert!(full.is_some());
    let part = NormRect::new(0.0, 0.0, 0.25, 0.25);
    let started = std::time::Instant::now();
    let frame = capturer.grab(part, Duration::from_secs(2)).expect("grab");
    assert!(frame.is_some());
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
#[ignore = "needs an interactive desktop"]
fn thumbnail_respects_max_width() {
    let frame = capture_thumbnail(None, 400).expect("thumbnail");
    assert!(frame.width <= 400 && frame.width > 0 && frame.height > 0);
    assert!(frame.view().is_valid());
}

#[test]
#[ignore = "needs an interactive desktop"]
fn thumbnail_works_while_a_capturer_is_active() {
    let mut capturer = Capturer::new(None, 64).expect("capturer");
    let thumb = capture_thumbnail(None, 320).expect("thumbnail via fallback");
    assert!(thumb.width <= 320 && thumb.width > 0);
    let frame = capturer
        .grab(NormRect::full(), Duration::from_secs(2))
        .expect("capturer still works");
    assert!(frame.is_some());
}

#[test]
#[ignore = "needs an interactive desktop"]
fn snapshot_reuses_the_active_session() {
    let mut capturer = Capturer::new(None, 16).expect("capturer");
    let small = NormRect::new(0.0, 0.0, 0.5, 0.5);
    let before = capturer.grab(small, Duration::from_secs(2)).expect("grab");
    let before = before.expect("frame");
    let snap = capturer
        .snapshot(500, Duration::from_secs(1))
        .expect("snapshot");
    assert!(snap.width <= 500 && snap.width > 100);
    // The snapshot covers the whole monitor, so it has the monitor's aspect ratio.
    let monitor = capturer.monitor();
    let expected = f64::from(monitor.width) / f64::from(monitor.height);
    let got = f64::from(snap.width) / f64::from(snap.height);
    assert!((expected - got).abs() / expected < 0.03);
    // The session keeps working and re-delivers right away.
    let started = std::time::Instant::now();
    let after = capturer.grab(small, Duration::from_secs(2)).expect("grab");
    assert!(after.is_some() && started.elapsed() < Duration::from_millis(500));
    assert_eq!(after.expect("frame").width, before.width);
}

#[test]
#[ignore = "needs an interactive desktop"]
fn unknown_monitor_is_reported() {
    assert!(matches!(
        Capturer::new(Some("mon-does-not-exist"), 64),
        Err(glowdrift_capture::CaptureError::MonitorNotFound)
    ));
}
