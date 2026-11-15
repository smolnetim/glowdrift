//! Opt-in per-stage timing of the engine loop, compiled in by the `engine-timing` feature.
//!
//! Without the feature every item here is an empty inline function and [`Mark`] is zero-sized, so
//! the engine carries no cost. With it, stage durations are accumulated and logged as one compact
//! summary line per interval, plus a final line when the engine stops.
//!
//! Stage durations are wall clock. CPU time comes from the thread cycle counter and is taken for
//! the engine thread as a whole, for the `Pass`, `Grab` and `Reduce` stages, and for the `Grab*`
//! sub-stages reported by the capturer; the other stages are too short to time that way.

pub use imp::Timing;

/// A measured part of the engine loop. The leaf stages are disjoint; the `Grab*` stages split
/// `Grab` and `Pass` covers a whole call of the engine's step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "engine-timing"), allow(dead_code))]
pub enum Stage {
    Pass,
    Resend,
    Grab,
    GrabAcquire,
    GrabStore,
    GrabSubmit,
    GrabMap,
    GrabCollect,
    GrabWaitGpu,
    GrabReadBack,
    Reduce,
    Target,
    Tick,
    Send,
    Publish,
    Error,
    Preview,
    Meter,
    Sleep,
}

#[cfg(feature = "engine-timing")]
mod imp {
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    use glowdrift_capture::GrabTimings;

    use super::Stage;

    const STAGE_COUNT: usize = Stage::Sleep as usize + 1;
    const DEFAULT_INTERVAL: Duration = Duration::from_secs(10);
    const INTERVAL_ENV: &str = "GLOWDRIFT_ENGINE_TIMING_SECS";

    /// Stages that make up a pass without overlapping each other.
    const LEAVES: [Stage; 9] = [
        Stage::Resend,
        Stage::Grab,
        Stage::Reduce,
        Stage::Target,
        Stage::Tick,
        Stage::Send,
        Stage::Publish,
        Stage::Error,
        Stage::Preview,
    ];
    /// Disjoint parts of `Grab`, as reported by the capturer.
    const GRAB_PARTS: [Stage; 7] = [
        Stage::GrabAcquire,
        Stage::GrabStore,
        Stage::GrabSubmit,
        Stage::GrabMap,
        Stage::GrabCollect,
        Stage::GrabWaitGpu,
        Stage::GrabReadBack,
    ];

    impl Stage {
        fn name(self) -> &'static str {
            match self {
                Stage::Pass => "pass",
                Stage::Resend => "resend",
                Stage::Grab => "grab",
                Stage::GrabAcquire => "grab.acquire",
                Stage::GrabStore => "grab.store",
                Stage::GrabSubmit => "grab.submit",
                Stage::GrabMap => "grab.map",
                Stage::GrabCollect => "grab.collect",
                Stage::GrabWaitGpu => "grab.wait_gpu",
                Stage::GrabReadBack => "grab.read_back",
                Stage::Reduce => "reduce",
                Stage::Target => "target",
                Stage::Tick => "tick",
                Stage::Send => "send",
                Stage::Publish => "publish",
                Stage::Error => "error",
                Stage::Preview => "preview",
                Stage::Meter => "meter",
                Stage::Sleep => "sleep",
            }
        }

        /// Whether the stage's CPU cycles are measured.
        fn has_cpu(self) -> bool {
            matches!(self, Stage::Pass | Stage::Grab | Stage::Reduce) || GRAB_PARTS.contains(&self)
        }
    }

    /// Start of a measured span; `cycles` is set when the span also measures CPU.
    #[derive(Debug, Clone, Copy)]
    pub struct Mark {
        at: Instant,
        cycles: Option<u64>,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct Stats {
        ns: [u64; STAGE_COUNT],
        cycles: [u64; STAGE_COUNT],
        calls: [u64; STAGE_COUNT],
        frames: u64,
        sleep_requested_ns: u64,
        wall_ns: u64,
        /// Cycles the engine thread ran over the window, from its own counter.
        thread_cycles: u64,
    }

    impl Stats {
        fn add(&mut self, other: &Stats) {
            for i in 0..STAGE_COUNT {
                self.ns[i] += other.ns[i];
                self.cycles[i] += other.cycles[i];
                self.calls[i] += other.calls[i];
            }
            self.frames += other.frames;
            self.sleep_requested_ns += other.sleep_requested_ns;
            self.wall_ns += other.wall_ns;
            self.thread_cycles += other.thread_cycles;
        }

        fn ns(&self, stage: Stage) -> u64 {
            self.ns[stage as usize]
        }

        fn cycles(&self, stage: Stage) -> u64 {
            self.cycles[stage as usize]
        }

        fn calls(&self, stage: Stage) -> u64 {
            self.calls[stage as usize]
        }
    }

    /// Cycles run so far by the calling thread; 0 where the counter is unavailable.
    fn cycles_now() -> u64 {
        #[cfg(windows)]
        {
            glowdrift_capture::cpu::thread_cycles()
        }
        #[cfg(not(windows))]
        {
            0
        }
    }

    /// Cycles per second of [`cycles_now`]; spins the calling thread for a few hundred
    /// milliseconds the first time.
    fn cycles_per_second() -> f64 {
        #[cfg(windows)]
        {
            glowdrift_capture::cpu::cycles_per_second()
        }
        #[cfg(not(windows))]
        {
            0.0
        }
    }

    fn ns_of(duration: Duration) -> u64 {
        u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
    }

    /// Accumulates stage timings and thread CPU time, and logs summaries.
    #[derive(Debug)]
    pub struct Timing {
        window: Stats,
        total: Stats,
        interval: Duration,
        window_start: Instant,
        next_report: Instant,
        cycles_at_window_start: u64,
        /// Cycles per second of the thread counter, set by `begin`.
        hz: f64,
    }

    impl Timing {
        pub const ENABLED: bool = true;

        pub fn new() -> Self {
            let interval = std::env::var(INTERVAL_ENV)
                .ok()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|secs| secs.is_finite() && *secs > 0.0)
                .map_or(DEFAULT_INTERVAL, Duration::from_secs_f64);
            let now = Instant::now();
            Self {
                window: Stats::default(),
                total: Stats::default(),
                interval,
                window_start: now,
                next_report: now + interval,
                cycles_at_window_start: 0,
                hz: 0.0,
            }
        }

        /// Calibrates the cycle counter and takes the CPU baseline. Must be called on the thread
        /// being measured.
        pub fn begin(&mut self) {
            self.hz = cycles_per_second();
            let now = Instant::now();
            self.window_start = now;
            self.next_report = now + self.interval;
            self.cycles_at_window_start = cycles_now();
            log::info!(
                "engine timing on: summary every {:.0} s, cycle counter {:.3} GHz",
                self.interval.as_secs_f64(),
                self.hz / 1e9
            );
        }

        /// Starts a span measured in wall clock only.
        pub fn start() -> Mark {
            Mark {
                at: Instant::now(),
                cycles: None,
            }
        }

        /// Starts a span measured in wall clock and thread CPU cycles.
        pub fn start_cpu() -> Mark {
            Mark {
                at: Instant::now(),
                cycles: Some(cycles_now()),
            }
        }

        pub fn record(&mut self, stage: Stage, mark: Mark) {
            let ns = ns_of(mark.at.elapsed());
            self.window.ns[stage as usize] += ns;
            self.window.calls[stage as usize] += 1;
            if let Some(start) = mark.cycles {
                self.window.cycles[stage as usize] += cycles_now().saturating_sub(start);
            }
        }

        /// Adds one sleep that was asked to last `requested`.
        pub fn record_sleep(&mut self, requested: Duration, mark: Mark) {
            self.window.sleep_requested_ns += ns_of(requested);
            self.record(Stage::Sleep, mark);
        }

        pub fn frame(&mut self) {
            self.window.frames += 1;
        }

        /// Adds the capturer's own split of the grab that just returned.
        pub fn grab_detail(&mut self, timings: GrabTimings) {
            let cpu = timings.cpu;
            for (stage, value, cycles) in [
                (Stage::GrabAcquire, timings.acquire, cpu.acquire),
                (Stage::GrabStore, timings.store, cpu.store),
                (Stage::GrabSubmit, timings.submit, cpu.submit),
                (Stage::GrabMap, timings.map, cpu.map),
                (Stage::GrabCollect, timings.collect_ready, cpu.collect_ready),
                (Stage::GrabWaitGpu, timings.wait_for_gpu, cpu.wait_for_gpu),
                (Stage::GrabReadBack, timings.read_back, cpu.read_back),
            ] {
                self.window.ns[stage as usize] += ns_of(value);
                self.window.cycles[stage as usize] += cycles;
                self.window.calls[stage as usize] += 1;
            }
        }

        /// Logs a summary when the interval has elapsed.
        pub fn poll(&mut self) {
            if Instant::now() >= self.next_report {
                self.close_window("window");
            }
        }

        /// Logs the summary of the whole run.
        pub fn finish(&mut self) {
            self.close_window("window");
            let line = summary_line("final", &self.total, self.hz);
            log::info!("{line}");
        }

        fn close_window(&mut self, label: &str) {
            let now = Instant::now();
            self.window.wall_ns = ns_of(now.saturating_duration_since(self.window_start));
            let cycles = cycles_now();
            self.window.thread_cycles = cycles.saturating_sub(self.cycles_at_window_start);
            self.cycles_at_window_start = cycles;
            if self.window.calls(Stage::Pass) > 0 {
                let line = summary_line(label, &self.window, self.hz);
                log::info!("{line}");
            }
            self.total.add(&self.window);
            self.window = Stats::default();
            self.window_start = now;
            self.next_report = now + self.interval;
        }
    }

    fn per_second(count: u64, wall_s: f64) -> f64 {
        count as f64 / wall_s
    }

    fn mean_us(ns: u64, calls: u64) -> f64 {
        if calls == 0 {
            0.0
        } else {
            ns as f64 / calls as f64 / 1e3
        }
    }

    /// One line: totals first, then `name wall-mean-us calls/s share-of-pass-time cpu-mean-us` per
    /// stage. `hz` converts cycles to time; the CPU mean is `-` for stages without CPU timing.
    fn summary_line(label: &str, stats: &Stats, hz: f64) -> String {
        let wall_s = (stats.wall_ns as f64 / 1e9).max(1e-9);
        let pass_ns = stats.ns(Stage::Pass).max(1);
        let share = |ns: u64| ns as f64 * 100.0 / pass_ns as f64;
        let cycle_ns = |cycles: u64| {
            if hz > 0.0 {
                cycles as f64 / hz * 1e9
            } else {
                0.0
            }
        };
        let cpu_pct = |cycles: u64| cycle_ns(cycles) * 100.0 / (wall_s * 1e9);

        let mut line = format!(
            "engine timing {label} {wall_s:.1}s: passes={} frames={} ticks={} wakes={} busy={:.2}%",
            stats.calls(Stage::Pass),
            stats.frames,
            stats.calls(Stage::Tick),
            stats.calls(Stage::Sleep),
            stats.ns(Stage::Pass) as f64 * 100.0 / (wall_s * 1e9),
        );
        let _ = write!(
            line,
            " cpu={:.2}% (in pass {:.2}%)",
            cpu_pct(stats.thread_cycles),
            cpu_pct(stats.cycles(Stage::Pass)),
        );
        if stats.frames > 0 {
            let _ = write!(
                line,
                " cpu/frame={:.0}us",
                cycle_ns(stats.thread_cycles) / stats.frames as f64 / 1e3
            );
        }
        line.push_str(" | stage wall_us calls/s share% cpu_us");

        let cpu_cell = |stage: Stage, cycles: u64| {
            if stage.has_cpu() {
                format!("{:.1}", mean_us_f(cycle_ns(cycles), stats.calls(stage)))
            } else {
                "-".to_owned()
            }
        };
        let entry = |line: &mut String, stage: Stage| {
            let _ = write!(
                line,
                " | {} {:.1} {:.1} {:.1} {}",
                stage.name(),
                mean_us(stats.ns(stage), stats.calls(stage)),
                per_second(stats.calls(stage), wall_s),
                share(stats.ns(stage)),
                cpu_cell(stage, stats.cycles(stage)),
            );
        };
        for stage in LEAVES {
            entry(&mut line, stage);
            if stage == Stage::Grab {
                for part in GRAB_PARTS {
                    entry(&mut line, part);
                }
                let parts_ns: u64 = GRAB_PARTS.iter().map(|part| stats.ns(*part)).sum();
                let parts_cycles: u64 = GRAB_PARTS.iter().map(|part| stats.cycles(*part)).sum();
                let rest_ns = stats.ns(Stage::Grab).saturating_sub(parts_ns);
                let rest_cycles = stats.cycles(Stage::Grab).saturating_sub(parts_cycles);
                let _ = write!(
                    line,
                    " | grab.other {:.1} {:.1} {:.1} {:.1}",
                    mean_us(rest_ns, stats.calls(Stage::Grab)),
                    per_second(stats.calls(Stage::Grab), wall_s),
                    share(rest_ns),
                    mean_us_f(cycle_ns(rest_cycles), stats.calls(Stage::Grab)),
                );
            }
        }
        entry(&mut line, Stage::Meter);
        let leaf_ns: u64 = LEAVES.iter().map(|leaf| stats.ns(*leaf)).sum();
        let other = stats
            .ns(Stage::Pass)
            .saturating_sub(leaf_ns + stats.ns(Stage::Meter));
        let _ = write!(
            line,
            " | other {:.1} {:.1} {:.1} -",
            mean_us(other, stats.calls(Stage::Pass)),
            per_second(stats.calls(Stage::Pass), wall_s),
            share(other),
        );
        let _ = write!(
            line,
            " | sleep {:.1} (asked {:.1}) {:.1}",
            mean_us(stats.ns(Stage::Sleep), stats.calls(Stage::Sleep)),
            mean_us(stats.sleep_requested_ns, stats.calls(Stage::Sleep)),
            per_second(stats.calls(Stage::Sleep), wall_s),
        );
        line
    }

    /// Like [`mean_us`] for a nanosecond total that is already a float.
    fn mean_us_f(ns: f64, calls: u64) -> f64 {
        if calls == 0 {
            0.0
        } else {
            ns / calls as f64 / 1e3
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn summary_reports_means_rates_shares_and_cpu() {
            let mut stats = Stats {
                wall_ns: 10_000_000_000,
                frames: 200,
                thread_cycles: 400_000_000,
                ..Stats::default()
            };
            stats.ns[Stage::Pass as usize] = 400_000_000;
            stats.cycles[Stage::Pass as usize] = 300_000_000;
            stats.calls[Stage::Pass as usize] = 400;
            stats.ns[Stage::Reduce as usize] = 100_000_000;
            stats.cycles[Stage::Reduce as usize] = 80_000_000;
            stats.calls[Stage::Reduce as usize] = 200;
            stats.ns[Stage::Tick as usize] = 6_000_000;
            stats.calls[Stage::Tick as usize] = 60;
            // 1 GHz makes one cycle one nanosecond.
            let line = summary_line("window", &stats, 1e9);
            assert!(line.contains("passes=400 frames=200"), "{line}");
            assert!(line.contains("cpu=4.00% (in pass 3.00%)"), "{line}");
            assert!(line.contains("cpu/frame=2000us"), "{line}");
            assert!(line.contains("| reduce 500.0 20.0 25.0 400.0"), "{line}");
            assert!(line.contains("| tick 100.0 6.0 1.5 -"), "{line}");
        }

        #[test]
        fn grab_other_is_the_part_not_reported_by_the_capturer() {
            let mut stats = Stats {
                wall_ns: 1_000_000_000,
                ..Stats::default()
            };
            stats.ns[Stage::Pass as usize] = 1_000_000;
            stats.calls[Stage::Pass as usize] = 1;
            stats.ns[Stage::Grab as usize] = 100_000;
            stats.cycles[Stage::Grab as usize] = 60_000;
            stats.calls[Stage::Grab as usize] = 1;
            stats.ns[Stage::GrabReadBack as usize] = 70_000;
            stats.cycles[Stage::GrabReadBack as usize] = 50_000;
            stats.calls[Stage::GrabReadBack as usize] = 1;
            let line = summary_line("window", &stats, 1e9);
            assert!(
                line.contains("| grab.read_back 70.0 1.0 7.0 50.0"),
                "{line}"
            );
            assert!(line.contains("| grab.other 30.0 1.0 3.0 10.0"), "{line}");
        }

        #[test]
        fn windows_fold_into_the_total() {
            let mut total = Stats::default();
            let mut window = Stats::default();
            window.ns[Stage::Tick as usize] = 5;
            window.calls[Stage::Tick as usize] = 1;
            window.frames = 2;
            window.thread_cycles = 7;
            total.add(&window);
            total.add(&window);
            assert_eq!(total.ns(Stage::Tick), 10);
            assert_eq!(total.frames, 4);
            assert_eq!(total.thread_cycles, 14);
        }
    }
}

#[cfg(not(feature = "engine-timing"))]
#[allow(clippy::unused_self)]
mod imp {
    use std::time::Duration;

    use glowdrift_capture::GrabTimings;

    use super::Stage;

    /// Zero-sized stand-in for the start of a measured span.
    #[derive(Debug, Clone, Copy)]
    pub struct Mark;

    /// Inert timer: every method is empty and `ENABLED` is false.
    #[derive(Debug)]
    pub struct Timing;

    impl Timing {
        pub const ENABLED: bool = false;

        #[inline]
        pub fn new() -> Self {
            Self
        }
        #[inline]
        pub fn begin(&mut self) {}
        #[inline]
        pub fn start() -> Mark {
            Mark
        }
        #[inline]
        pub fn start_cpu() -> Mark {
            Mark
        }
        #[inline]
        pub fn record(&mut self, _stage: Stage, _mark: Mark) {}
        #[inline]
        pub fn record_sleep(&mut self, _requested: Duration, _mark: Mark) {}
        #[inline]
        pub fn frame(&mut self) {}
        #[inline]
        pub fn grab_detail(&mut self, _timings: GrabTimings) {}
        #[inline]
        pub fn poll(&mut self) {}
        #[inline]
        pub fn finish(&mut self) {}
    }
}
