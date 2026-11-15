//! Send scheduling for the Hue light: at most one PUT per [`PUT_GAP`] across all lights, latest
//! colour wins, nothing is queued.
//!
//! Every new colour makes all lights stale. The pacer hands out stale lights round-robin, one
//! per slot, so with `n` lights each light is refreshed once per `n * PUT_GAP`. That period is
//! also the transition duration, so a light fades until its next update arrives. The pacer is
//! pure: callers pass the current time.

use std::time::{Duration, Instant};

/// Minimum gap between two light PUTs (10 per second in total).
pub(crate) const PUT_GAP: Duration = Duration::from_millis(100);

/// What the worker should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Poll {
    /// Send the current colour to the light at this index now.
    Send(usize),
    /// A light is stale but the rate budget is spent; ask again at this time.
    WaitUntil(Instant),
    /// Every light is up to date.
    Idle,
}

#[derive(Debug)]
pub(crate) struct Pacer {
    gap: Duration,
    generation: u64,
    /// Generation last delivered (or deliberately skipped) per light.
    delivered: Vec<u64>,
    cursor: usize,
    next_slot: Option<Instant>,
}

impl Pacer {
    pub fn new(gap: Duration) -> Pacer {
        Pacer {
            gap,
            generation: 0,
            delivered: Vec::new(),
            cursor: 0,
            next_slot: None,
        }
    }

    /// Replaces the light set; every light is stale if a colour exists.
    pub fn set_lights(&mut self, count: usize) {
        self.delivered = vec![0; count];
        self.cursor = 0;
    }

    /// Records a new desired colour: all lights become stale.
    pub fn touch(&mut self) {
        self.generation += 1;
    }

    pub fn poll(&mut self, now: Instant) -> Poll {
        let count = self.delivered.len();
        let Some(index) = (0..count)
            .map(|offset| (self.cursor + offset) % count)
            .find(|&i| self.delivered[i] != self.generation)
        else {
            return Poll::Idle;
        };
        match self.next_slot {
            Some(at) if at > now => Poll::WaitUntil(at),
            _ => Poll::Send(index),
        }
    }

    /// A PUT for `index` was attempted at `now` (successfully or not); it uses a slot.
    pub fn attempted(&mut self, index: usize, now: Instant) {
        self.delivered[index] = self.generation;
        self.cursor = (index + 1) % self.delivered.len().max(1);
        self.next_slot = Some(now + self.gap);
    }

    /// The light already shows the right output; no slot is used.
    pub fn skipped(&mut self, index: usize) {
        self.delivered[index] = self.generation;
        self.cursor = (index + 1) % self.delivered.len().max(1);
    }
}

pub(crate) fn round_interval(gap: Duration, lights: usize) -> Duration {
    gap * u32::try_from(lights.max(1)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(base: Instant, n: u64) -> Instant {
        base + Duration::from_millis(n)
    }

    #[test]
    fn idle_without_colour_or_lights() {
        let mut pacer = Pacer::new(PUT_GAP);
        let t0 = Instant::now();
        assert_eq!(pacer.poll(t0), Poll::Idle);
        pacer.set_lights(3);
        assert_eq!(pacer.poll(t0), Poll::Idle);
    }

    #[test]
    fn first_send_is_immediate_then_one_per_gap() {
        let mut pacer = Pacer::new(PUT_GAP);
        pacer.set_lights(3);
        pacer.touch();
        let t0 = Instant::now();
        assert_eq!(pacer.poll(t0), Poll::Send(0));
        pacer.attempted(0, t0);
        assert_eq!(pacer.poll(ms(t0, 10)), Poll::WaitUntil(ms(t0, 100)));
        assert_eq!(pacer.poll(ms(t0, 100)), Poll::Send(1));
        pacer.attempted(1, ms(t0, 100));
        assert_eq!(pacer.poll(ms(t0, 200)), Poll::Send(2));
        pacer.attempted(2, ms(t0, 200));
        assert_eq!(pacer.poll(ms(t0, 400)), Poll::Idle);
    }

    #[test]
    fn never_exceeds_ten_per_second() {
        let mut pacer = Pacer::new(PUT_GAP);
        pacer.set_lights(4);
        let t0 = Instant::now();
        let mut sends = Vec::new();
        let mut now = t0;
        // A new colour every 5 ms for 3 s of simulated time.
        for tick in 0..600u64 {
            now = ms(t0, tick * 5);
            pacer.touch();
            while let Poll::Send(i) = pacer.poll(now) {
                pacer.attempted(i, now);
                sends.push(now);
            }
        }
        let _ = now;
        for pair in sends.windows(2) {
            assert!(pair[1] - pair[0] >= PUT_GAP);
        }
        assert!(sends.len() <= 31, "{} sends in 3 s", sends.len());
        assert!(sends.len() >= 29);
    }

    #[test]
    fn newest_colour_replaces_the_pending_one_and_nothing_queues() {
        let mut pacer = Pacer::new(PUT_GAP);
        pacer.set_lights(1);
        let t0 = Instant::now();
        pacer.touch();
        assert_eq!(pacer.poll(t0), Poll::Send(0));
        pacer.attempted(0, t0);
        // Ten colours arrive during the wait; only one send is owed afterwards.
        for _ in 0..10 {
            pacer.touch();
        }
        assert_eq!(pacer.poll(ms(t0, 100)), Poll::Send(0));
        pacer.attempted(0, ms(t0, 100));
        assert_eq!(pacer.poll(ms(t0, 1000)), Poll::Idle);
    }

    #[test]
    fn lights_are_served_round_robin_across_colours() {
        let mut pacer = Pacer::new(PUT_GAP);
        pacer.set_lights(3);
        let t0 = Instant::now();
        pacer.touch();
        pacer.attempted(0, t0);
        // A fresh colour must continue with light 1, not restart at 0.
        pacer.touch();
        assert_eq!(pacer.poll(ms(t0, 100)), Poll::Send(1));
    }

    #[test]
    fn skipped_lights_use_no_slot() {
        let mut pacer = Pacer::new(PUT_GAP);
        pacer.set_lights(2);
        pacer.touch();
        let t0 = Instant::now();
        assert_eq!(pacer.poll(t0), Poll::Send(0));
        pacer.skipped(0);
        assert_eq!(pacer.poll(t0), Poll::Send(1));
    }

    #[test]
    fn round_interval_scales_with_light_count() {
        assert_eq!(round_interval(PUT_GAP, 0), PUT_GAP);
        assert_eq!(round_interval(PUT_GAP, 1), PUT_GAP);
        assert_eq!(round_interval(PUT_GAP, 7), Duration::from_millis(700));
    }
}
