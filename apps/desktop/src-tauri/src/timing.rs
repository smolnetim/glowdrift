//! Opt-in per-stage timing of the engine loop, compiled in by the `engine-timing` feature.
//!
//! Without the feature every item here is an empty inline function and [`Mark`] is zero-sized, so
//! the engine carries no cost. With it, stage durations are accumulated and logged as one compact
//! summary line per interval, plus a final line when the engine stops.

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
    const GRAB_PARTS: [Stage; 4] = [
        Stage::GrabAcquire,
        Stage::GrabStore,
        Stage::GrabSubmit,
        Stage::GrabMap,
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
    }

    /// Start of a measured span.
    #[derive(Debug, Clone, Copy)]
    pub struct Mark(Instant);

    #[derive(Debug, Clone, Copy, Default)]
    struct Cpu {
        user_ns: u64,
        kernel_ns: u64,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct Stats {
        ns: [u64; STAGE_COUNT],
        calls: [u64; STAGE_COUNT],
        frames: u64,
        sleep_requested_ns: u64,
        wall_ns: u64,
        cpu: Cpu,
    }

    impl Stats {
        fn add(&mut self, other: &Stats) {
            for i in 0..STAGE_COUNT {
                self.ns[i] += other.ns[i];
                self.calls[i] += other.calls[i];
            }
            self.frames += other.frames;
            self.sleep_requested_ns += other.sleep_requested_ns;
            self.wall_ns += other.wall_ns;
            self.cpu.user_ns += other.cpu.user_ns;
            self.cpu.kernel_ns += other.cpu.kernel_ns;
        }

        fn ns(&self, stage: Stage) -> u64 {
            self.ns[stage as usize]
        }

        fn calls(&self, stage: Stage) -> u64 {
            self.calls[stage as usize]
        }
    }

    #[cfg(windows)]
    fn thread_cpu() -> Option<Cpu> {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

        let (mut created, mut exited) = (FILETIME::default(), FILETIME::default());
        let (mut kernel, mut user) = (FILETIME::default(), FILETIME::default());
        // SAFETY: the pseudo handle of the current thread is always valid and the out
        // pointers refer to live locals.
        unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            )
        }
        .ok()?;
        // FILETIME counts 100 ns units.
        let ns =
            |t: FILETIME| ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) * 100;
        Some(Cpu {
            user_ns: ns(user),
            kernel_ns: ns(kernel),
        })
    }

    #[cfg(not(windows))]
    fn thread_cpu() -> Option<Cpu> {
        None
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
        cpu_at_window_start: Option<Cpu>,
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
                cpu_at_window_start: None,
            }
        }

        /// Takes the CPU baseline. Must be called on the thread being measured.
        pub fn begin(&mut self) {
            let now = Instant::now();
            self.window_start = now;
            self.next_report = now + self.interval;
            self.cpu_at_window_start = thread_cpu();
            log::info!(
                "engine timing on: summary every {:.0} s",
                self.interval.as_secs_f64()
            );
        }

        pub fn start() -> Mark {
            Mark(Instant::now())
        }

        pub fn record(&mut self, stage: Stage, mark: Mark) {
            let ns = ns_of(mark.0.elapsed());
            self.window.ns[stage as usize] += ns;
            self.window.calls[stage as usize] += 1;
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
            for (stage, value) in [
                (Stage::GrabAcquire, timings.acquire),
                (Stage::GrabStore, timings.store),
                (Stage::GrabSubmit, timings.submit),
                (Stage::GrabMap, timings.map),
            ] {
                self.window.ns[stage as usize] += ns_of(value);
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
            let line = summary_line("final", &self.total);
            log::info!("{line}");
        }

        fn close_window(&mut self, label: &str) {
            let now = Instant::now();
            self.window.wall_ns = ns_of(now.saturating_duration_since(self.window_start));
            let cpu = thread_cpu();
            if let (Some(before), Some(after)) = (self.cpu_at_window_start, cpu) {
                self.window.cpu = Cpu {
                    user_ns: after.user_ns.saturating_sub(before.user_ns),
                    kernel_ns: after.kernel_ns.saturating_sub(before.kernel_ns),
                };
            }
            self.cpu_at_window_start = cpu;
            if self.window.calls(Stage::Pass) > 0 {
                let line = summary_line(label, &self.window);
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

    /// One line: totals first, then `name mean-per-call rate share-of-pass-time` per stage.
    fn summary_line(label: &str, stats: &Stats) -> String {
        let wall_s = (stats.wall_ns as f64 / 1e9).max(1e-9);
        let pass_ns = stats.ns(Stage::Pass).max(1);
        let share = |ns: u64| ns as f64 * 100.0 / pass_ns as f64;
        let cpu_ns = stats.cpu.user_ns + stats.cpu.kernel_ns;
        let cpu_pct = |ns: u64| ns as f64 * 100.0 / (wall_s * 1e9);

        let mut line = format!(
            "engine timing {label} {wall_s:.1}s: passes={} frames={} ticks={} wakes={} busy={:.2}%",
            stats.calls(Stage::Pass),
            stats.frames,
            stats.calls(Stage::Tick),
            stats.calls(Stage::Sleep),
            cpu_pct(stats.ns(Stage::Pass)),
        );
        let _ = write!(
            line,
            " cpu={:.2}% (user {:.2}% kernel {:.2}%)",
            cpu_pct(cpu_ns),
            cpu_pct(stats.cpu.user_ns),
            cpu_pct(stats.cpu.kernel_ns),
        );
        if stats.frames > 0 {
            let _ = write!(
                line,
                " cpu/frame={:.0}us",
                cpu_ns as f64 / stats.frames as f64 / 1e3
            );
        }
        line.push_str(" | stage mean_us calls/s share%");

        let entry = |line: &mut String, stage: Stage| {
            let _ = write!(
                line,
                " | {} {:.1} {:.1} {:.1}",
                stage.name(),
                mean_us(stats.ns(stage), stats.calls(stage)),
                per_second(stats.calls(stage), wall_s),
                share(stats.ns(stage)),
            );
        };
        for stage in LEAVES {
            entry(&mut line, stage);
            if stage == Stage::Grab {
                let parts: u64 = GRAB_PARTS.iter().map(|part| stats.ns(*part)).sum();
                for part in GRAB_PARTS {
                    entry(&mut line, part);
                }
                let rest = stats.ns(Stage::Grab).saturating_sub(parts);
                let _ = write!(
                    line,
                    " | grab.other {:.1} {:.1} {:.1}",
                    mean_us(rest, stats.calls(Stage::Grab)),
                    per_second(stats.calls(Stage::Grab), wall_s),
                    share(rest),
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
            " | other {:.1} {:.1} {:.1}",
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn summary_reports_means_rates_and_shares() {
            let mut stats = Stats {
                wall_ns: 10_000_000_000,
                frames: 200,
                ..Stats::default()
            };
            stats.ns[Stage::Pass as usize] = 400_000_000;
            stats.calls[Stage::Pass as usize] = 400;
            stats.ns[Stage::Reduce as usize] = 100_000_000;
            stats.calls[Stage::Reduce as usize] = 200;
            stats.cpu = Cpu {
                user_ns: 300_000_000,
                kernel_ns: 100_000_000,
            };
            let line = summary_line("window", &stats);
            assert!(line.contains("passes=400 frames=200"), "{line}");
            assert!(line.contains("cpu=4.00%"), "{line}");
            assert!(line.contains("cpu/frame=2000us"), "{line}");
            assert!(line.contains("| reduce 500.0 20.0 25.0"), "{line}");
        }

        #[test]
        fn windows_fold_into_the_total() {
            let mut total = Stats::default();
            let mut window = Stats::default();
            window.ns[Stage::Tick as usize] = 5;
            window.calls[Stage::Tick as usize] = 1;
            window.frames = 2;
            total.add(&window);
            total.add(&window);
            assert_eq!(total.ns(Stage::Tick), 10);
            assert_eq!(total.frames, 4);
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
