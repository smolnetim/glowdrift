//! Temporal exponential smoothing in linear light.

use std::time::Duration;

use crate::color::LinearRgb;

/// Exponential smoother: each update moves the state towards the target by
/// `1 - exp(-dt / time_constant)` of the remaining distance, so the result is independent
/// of the tick rate.
#[derive(Debug, Clone, Default)]
pub struct Smoother {
    state: Option<LinearRgb>,
}

impl Smoother {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the current state; the next update snaps to its target.
    pub fn reset(&mut self) {
        self.state = None;
    }

    /// The current smoothed colour, or `None` before the first valid update.
    pub fn current(&self) -> Option<LinearRgb> {
        self.state
    }

    /// Advances the smoother by `dt` towards `target` and returns the new colour.
    ///
    /// The first valid target, and any target with a zero `time_constant`, is returned
    /// immediately. A target with a non-finite channel is ignored: the state is left
    /// unchanged and returned (black if there is no state yet).
    pub fn update(
        &mut self,
        target: LinearRgb,
        dt: Duration,
        time_constant: Duration,
    ) -> LinearRgb {
        if !(target.r.is_finite() && target.g.is_finite() && target.b.is_finite()) {
            return self.state.unwrap_or(LinearRgb::BLACK);
        }
        let next = match self.state {
            Some(current) if !time_constant.is_zero() => {
                let ratio = dt.as_secs_f64() / time_constant.as_secs_f64();
                let alpha = (1.0 - (-ratio).exp()) as f32;
                let lerp = |from: f32, to: f32| from + (to - from) * alpha;
                LinearRgb::new(
                    lerp(current.r, target.r),
                    lerp(current.g, target.g),
                    lerp(current.b, target.b),
                )
            }
            _ => target,
        };
        self.state = Some(next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: LinearRgb = LinearRgb::new(1.0, 1.0, 1.0);

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    fn step_response(step_ms: u64, total_ms: u64, tau_ms: u64) -> f32 {
        let mut s = Smoother::new();
        s.update(LinearRgb::BLACK, ms(step_ms), ms(tau_ms));
        let mut out = LinearRgb::BLACK;
        for _ in 0..total_ms / step_ms {
            out = s.update(WHITE, ms(step_ms), ms(tau_ms));
        }
        out.r
    }

    #[test]
    fn first_sample_snaps() {
        let mut s = Smoother::new();
        assert_eq!(s.current(), None);
        let c = LinearRgb::new(0.2, 0.4, 0.6);
        assert_eq!(s.update(c, ms(16), ms(500)), c);
        assert_eq!(s.current(), Some(c));
    }

    #[test]
    fn reset_makes_next_update_snap() {
        let mut s = Smoother::new();
        s.update(LinearRgb::BLACK, ms(16), ms(500));
        s.reset();
        assert_eq!(s.current(), None);
        assert_eq!(s.update(WHITE, ms(16), ms(500)), WHITE);
    }

    #[test]
    fn zero_time_constant_snaps() {
        let mut s = Smoother::new();
        s.update(LinearRgb::BLACK, ms(16), ms(500));
        assert_eq!(s.update(WHITE, ms(16), Duration::ZERO), WHITE);
    }

    #[test]
    fn step_reaches_63_percent_after_one_time_constant() {
        let expected = 1.0 - (-1.0_f32).exp();
        for step in [1, 4, 8, 10, 20, 50, 100, 250] {
            let got = step_response(step, 1000, 1000);
            assert!(
                (got - expected).abs() < 1e-4,
                "step {step} ms gave {got}, expected {expected}"
            );
        }
        assert!((expected - 0.632).abs() < 1e-3);
    }

    #[test]
    fn step_response_is_independent_of_tick_rate() {
        let a = step_response(10, 600, 400);
        let b = step_response(60, 600, 400);
        let c = step_response(300, 600, 400);
        assert!((a - b).abs() < 1e-4 && (a - c).abs() < 1e-4, "{a} {b} {c}");
    }

    #[test]
    fn zero_dt_changes_nothing() {
        let mut s = Smoother::new();
        s.update(LinearRgb::new(0.3, 0.3, 0.3), ms(16), ms(500));
        let out = s.update(WHITE, Duration::ZERO, ms(500));
        assert_eq!(out, LinearRgb::new(0.3, 0.3, 0.3));
    }

    #[test]
    fn huge_dt_reaches_target() {
        let mut s = Smoother::new();
        s.update(LinearRgb::BLACK, ms(16), ms(500));
        let out = s.update(WHITE, Duration::MAX, ms(500));
        assert_eq!(out, WHITE);
        let out = s.update(
            LinearRgb::BLACK,
            Duration::from_secs(1_000_000),
            Duration::from_nanos(1),
        );
        assert_eq!(out, LinearRgb::BLACK);
    }

    #[test]
    fn huge_time_constant_barely_moves() {
        let mut s = Smoother::new();
        s.update(LinearRgb::BLACK, ms(16), ms(500));
        let out = s.update(WHITE, ms(16), Duration::MAX);
        assert!(out.r >= 0.0 && out.r < 1e-6);
    }

    #[test]
    fn non_finite_targets_are_ignored() {
        let mut s = Smoother::new();
        let bad = LinearRgb::new(f32::NAN, 0.5, 0.5);
        assert_eq!(s.update(bad, ms(16), ms(500)), LinearRgb::BLACK);
        assert_eq!(s.current(), None);

        let c = LinearRgb::new(0.2, 0.2, 0.2);
        s.update(c, ms(16), ms(500));
        assert_eq!(
            s.update(LinearRgb::new(0.0, f32::INFINITY, 0.0), ms(16), ms(500)),
            c
        );
        assert_eq!(s.current(), Some(c));
    }

    #[test]
    fn converges_monotonically_without_overshoot() {
        let mut s = Smoother::new();
        s.update(LinearRgb::BLACK, ms(16), ms(300));
        let mut prev = 0.0;
        for _ in 0..500 {
            let v = s.update(WHITE, ms(16), ms(300)).r;
            assert!(v >= prev && v <= 1.0);
            prev = v;
        }
        assert!(prev > 0.999);
    }

    #[test]
    fn channels_are_independent() {
        let mut s = Smoother::new();
        s.update(LinearRgb::BLACK, ms(16), ms(500));
        let out = s.update(LinearRgb::new(1.0, 0.0, 0.5), ms(100), ms(100));
        let a = 1.0 - (-1.0_f32).exp();
        assert!((out.r - a).abs() < 1e-5);
        assert_eq!(out.g, 0.0);
        assert!((out.b - 0.5 * a).abs() < 1e-5);
    }
}
