//! CPU time from the per-thread cycle counters (Windows).
//!
//! `GetThreadTimes` is sampled by the scheduler tick (about 15.6 ms), so it misses most of a
//! thread that wakes briefly and often. `QueryThreadCycleTime` counts the cycles the thread
//! actually ran, so it is exact but needs a cycles-per-second figure to become time.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::System::Threading::GetCurrentThread;
use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;

const CALIBRATION_ROUNDS: usize = 6;
const CALIBRATION_SPIN: Duration = Duration::from_millis(50);

/// Cycles the calling thread has run so far, or 0 if the query fails.
pub fn thread_cycles() -> u64 {
    let mut cycles = 0u64;
    // SAFETY: the pseudo handle of the current thread is always valid and the out parameter is a
    // live local.
    let _ = unsafe { QueryThreadCycleTime(GetCurrentThread(), &raw mut cycles) };
    cycles
}

/// Cycles per second of the counter behind [`thread_cycles`], measured once and cached.
///
/// A thread that is descheduled stops accumulating cycles but not wall time, so a spin that is
/// interrupted reads low and never high. The result is the highest rate over several short spins
/// of the calling thread, which keeps one undisturbed round.
pub fn cycles_per_second() -> f64 {
    static RATE: OnceLock<f64> = OnceLock::new();
    *RATE.get_or_init(|| {
        (0..CALIBRATION_ROUNDS)
            .map(|_| {
                let (cycles, start) = (thread_cycles(), Instant::now());
                while start.elapsed() < CALIBRATION_SPIN {
                    std::hint::spin_loop();
                }
                let wall = start.elapsed().as_secs_f64();
                thread_cycles().saturating_sub(cycles) as f64 / wall
            })
            .fold(0.0, f64::max)
    })
}

/// Converts a cycle count to a duration using [`cycles_per_second`].
pub fn cycles_to_duration(cycles: u64) -> Duration {
    Duration::from_secs_f64(cycles as f64 / cycles_per_second())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_plausible_and_cycles_advance() {
        let before = thread_cycles();
        let rate = cycles_per_second();
        assert!((0.5e9..8e9).contains(&rate), "{rate}");
        assert!(thread_cycles() > before);
    }
}
