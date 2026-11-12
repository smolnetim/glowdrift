//! Prints the monitors, then captures one monitor for a few seconds and reports grab timings.
//!
//! Usage: `probe [--monitor <id>] [--seconds <n>] [--fps <n>] [--timeout-ms <n>] [--max-dim <px>]
//! [--region x,y,w,h]`. `--fps 0` grabs back to back, which measures latency from a screen change.

use std::time::{Duration, Instant};

use glowdrift_capture::{
    CaptureError, Capturer, Frame, GrabTimings, capture_thumbnail, list_monitors,
};
use glowdrift_core::{NormRect, ReduceMode, reduce};

struct Args {
    monitor: Option<String>,
    seconds: f32,
    fps: u32,
    /// Grab timeout in milliseconds; defaults to the frame period.
    timeout_ms: Option<u64>,
    max_dim: u32,
    region: NormRect,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        monitor: None,
        seconds: 3.0,
        fps: 20,
        timeout_ms: None,
        max_dim: 64,
        region: NormRect::full(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--monitor" => args.monitor = Some(value()?),
            "--seconds" => args.seconds = value()?.parse().map_err(|e| format!("{e}"))?,
            "--fps" => args.fps = value()?.parse().map_err(|e| format!("{e}"))?,
            "--timeout-ms" => args.timeout_ms = Some(value()?.parse().map_err(|e| format!("{e}"))?),
            "--max-dim" => args.max_dim = value()?.parse().map_err(|e| format!("{e}"))?,
            "--region" => {
                let v: Vec<f32> = value()?
                    .split(',')
                    .map(|p| p.trim().parse::<f32>().map_err(|e| format!("{e}")))
                    .collect::<Result<_, _>>()?;
                let [x, y, w, h] = v[..] else {
                    return Err("--region needs x,y,w,h".into());
                };
                args.region = NormRect::new(x, y, w, h);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

/// Mean of the sRGB bytes; independent of glowdrift-core so it works while `reduce` is a stub.
fn byte_average_hex(frame: &Frame) -> String {
    let view = frame.view();
    let (mut b, mut g, mut r, mut n) = (0u64, 0u64, 0u64, 0u64);
    for y in 0..view.height {
        if let Some(row) = view.row(y) {
            for px in row.as_chunks::<4>().0 {
                b += u64::from(px[0]);
                g += u64::from(px[1]);
                r += u64::from(px[2]);
                n += 1;
            }
        }
    }
    let n = n.max(1);
    format!("#{:02x}{:02x}{:02x}", r / n, g / n, b / n)
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index]
}

struct Run {
    grab_times: Vec<Duration>,
    /// Thread CPU time spent inside each delivered `grab` call.
    cpu_times: Vec<Duration>,
    timings: Vec<GrabTimings>,
    timeouts: u32,
    last: Option<Frame>,
    /// Process CPU time (user + kernel) and wall time over the whole run.
    process_cpu: Duration,
    /// Process CPU time from the cycle counter, which counts every thread precisely.
    process_cycle_cpu: Duration,
    wall: Duration,
}

fn thread_cycles() -> u64 {
    use windows::Win32::System::Threading::GetCurrentThread;
    use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;
    let mut cycles = 0u64;
    // SAFETY: the pseudo handle is always valid and the out parameter is a live local.
    let _ = unsafe { QueryThreadCycleTime(GetCurrentThread(), &raw mut cycles) };
    cycles
}

/// Thread cycles per second, measured by spinning briefly.
fn cycles_per_second() -> f64 {
    let (c0, t0) = (thread_cycles(), Instant::now());
    while t0.elapsed() < Duration::from_millis(150) {
        std::hint::spin_loop();
    }
    (thread_cycles() - c0) as f64 / t0.elapsed().as_secs_f64()
}

fn process_cycles() -> u64 {
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::Win32::System::WindowsProgramming::QueryProcessCycleTime;
    let mut cycles = 0u64;
    // SAFETY: the pseudo handle is always valid and the out parameter is a live local.
    let _ = unsafe { QueryProcessCycleTime(GetCurrentProcess(), &raw mut cycles) };
    cycles
}

fn process_cpu() -> Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = FILETIME::default();
    let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo handle is always valid and the out parameters are live locals.
    let _ = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

fn run_capture(capturer: &mut Capturer, target: Option<&str>, args: &Args) -> Run {
    let period = if args.fps == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(1.0 / f64::from(args.fps))
    };
    let timeout = args.timeout_ms.map_or_else(
        || {
            if period.is_zero() {
                Duration::from_millis(100)
            } else {
                period
            }
        },
        Duration::from_millis,
    );
    let start = Instant::now();
    let end = start + Duration::from_secs_f32(args.seconds);
    let mut grab_times: Vec<Duration> = Vec::new();
    let mut timeouts = 0u32;
    let mut last: Option<Frame> = None;
    let mut next_tick = start;
    let hz = cycles_per_second();
    let mut cpu_times: Vec<Duration> = Vec::new();
    let mut timings: Vec<GrabTimings> = Vec::new();
    let cpu_start = process_cpu();
    let pc_start = process_cycles();
    let wall_start = Instant::now();
    while Instant::now() < end {
        let begin = Instant::now();
        let cycles = thread_cycles();
        let result = capturer.grab(args.region, timeout);
        let used = thread_cycles() - cycles;
        match result {
            Ok(Some(frame)) => {
                grab_times.push(begin.elapsed());
                cpu_times.push(Duration::from_secs_f64(used as f64 / hz));
                timings.push(capturer.last_timings());
                last = Some(frame);
            }
            Ok(None) => timeouts += 1,
            Err(CaptureError::AccessLost) => {
                eprintln!("access lost; recreating capturer");
                match Capturer::new(target, args.max_dim) {
                    Ok(c) => *capturer = c,
                    Err(e) => {
                        eprintln!("recreate failed: {e}");
                        break;
                    }
                }
            }
            Err(e) => {
                eprintln!("grab failed: {e}");
                break;
            }
        }
        next_tick += period;
        if let Some(wait) = next_tick.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    Run {
        grab_times,
        cpu_times,
        timings,
        timeouts,
        last,
        process_cpu: process_cpu().saturating_sub(cpu_start),
        process_cycle_cpu: Duration::from_secs_f64((process_cycles() - pc_start) as f64 / hz),
        wall: wall_start.elapsed(),
    }
}

fn report(run: &Run) {
    let mut grab_times = run.grab_times.clone();
    let (timeouts, last) = (run.timeouts, &run.last);
    println!(
        "\nframes delivered: {}, no-change polls: {}",
        grab_times.len(),
        timeouts
    );
    if !grab_times.is_empty() {
        grab_times.sort();
        let total: Duration = grab_times.iter().sum();
        println!(
            "grab duration (frames only): min {:.2?}  avg {:.2?}  p95 {:.2?}  max {:.2?}",
            grab_times[0],
            total / grab_times.len() as u32,
            percentile(&grab_times, 0.95),
            grab_times[grab_times.len() - 1]
        );
    }
    if !run.timings.is_empty() {
        let n = run.timings.len() as u32;
        let avg = |f: fn(&GrabTimings) -> Duration| run.timings.iter().map(f).sum::<Duration>() / n;
        let cpu: Duration = run.cpu_times.iter().sum::<Duration>() / n;
        let mut cpu_sorted = run.cpu_times.clone();
        cpu_sorted.sort();
        println!(
            "thread CPU per delivered frame: avg {:.2?}  p95 {:.2?}  max {:.2?}",
            cpu,
            percentile(&cpu_sorted, 0.95),
            cpu_sorted[cpu_sorted.len() - 1]
        );
        println!(
            "avg phases per grab: acquire(wait) {:.2?}  store {:.2?}  submit {:.2?}  map-block {:.2?}",
            avg(|t| t.acquire),
            avg(|t| t.store),
            avg(|t| t.submit),
            avg(|t| t.map)
        );
        let mut latencies: Vec<Duration> = run.timings.iter().filter_map(|t| t.latency).collect();
        latencies.sort();
        if !latencies.is_empty() {
            let total: Duration = latencies.iter().sum();
            println!(
                "latency present->returned: min {:.2?}  avg {:.2?}  p95 {:.2?}  max {:.2?} ({} samples)",
                latencies[0],
                total / latencies.len() as u32,
                percentile(&latencies, 0.95),
                latencies[latencies.len() - 1],
                latencies.len()
            );
        }
    }
    println!(
        "process CPU over run: {:.2?} of {:.2?} wall = {:.2}% of one core (cycle counter: {:.2?} = {:.2}%)",
        run.process_cpu,
        run.wall,
        run.process_cpu.as_secs_f64() / run.wall.as_secs_f64() * 100.0,
        run.process_cycle_cpu,
        run.process_cycle_cpu.as_secs_f64() / run.wall.as_secs_f64() * 100.0
    );
    if let Some(frame) = last {
        let color = reduce(&frame.view(), ReduceMode::Average)
            .to_srgb()
            .to_hex();
        println!(
            "last frame: {}x{} stride {}",
            frame.width, frame.height, frame.stride
        );
        println!("average colour (glowdrift-core reduce): {color}");
        println!(
            "average colour (probe byte mean):       {}",
            byte_average_hex(frame)
        );
    }
}

fn processing_cost(capturer: &mut Capturer) {
    println!("\nprocessing cost (cached desktop image, alternating regions):");
    let mut costs: Vec<Duration> = Vec::new();
    for i in 0..200 {
        let half = if i % 2 == 0 {
            NormRect::new(0.0, 0.0, 0.5, 0.5)
        } else {
            NormRect::new(0.5, 0.5, 0.5, 0.5)
        };
        let t = Instant::now();
        if let Err(e) = capturer.grab(half, Duration::ZERO) {
            println!("  failed: {e}");
            break;
        }
        costs.push(t.elapsed());
    }
    costs.sort();
    if !costs.is_empty() {
        let total: Duration = costs.iter().sum();
        println!(
            "  min {:.2?}  avg {:.2?}  p95 {:.2?}  max {:.2?}  ({} grabs)",
            costs[0],
            total / costs.len() as u32,
            percentile(&costs, 0.95),
            costs[costs.len() - 1],
            costs.len()
        );
    }
}

fn main() {
    // Physical pixel coordinates and GDI capture need per-monitor DPI awareness.
    // SAFETY: called once at startup before any window exists.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    let monitors = match list_monitors() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("list_monitors failed: {e}");
            std::process::exit(1);
        }
    };
    println!("monitors ({}):", monitors.len());
    for m in &monitors {
        println!(
            "  {} {:<28} {}x{} at ({}, {}) scale {:.2}{}",
            m.id,
            m.name,
            m.width,
            m.height,
            m.x,
            m.y,
            m.scale_factor,
            if m.primary { " [primary]" } else { "" }
        );
    }

    let target = args.monitor.as_deref();
    println!(
        "\ncapturing {} for {:.1}s at {} fps, max_dimension {}, region {:?}",
        target.unwrap_or("primary monitor"),
        args.seconds,
        args.fps,
        args.max_dim,
        args.region
    );

    let created = Instant::now();
    let mut capturer = match Capturer::new(target, args.max_dim) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Capturer::new failed: {e}");
            std::process::exit(1);
        }
    };
    println!("capturer created in {:.1?}", created.elapsed());

    let run = run_capture(&mut capturer, target, &args);
    report(&run);

    processing_cost(&mut capturer);

    let t = Instant::now();
    match capturer.snapshot(640, Duration::from_secs(1)) {
        Ok(frame) => println!(
            "\nsnapshot from active session: {}x{} in {:.1?} (byte mean {})",
            frame.width,
            frame.height,
            t.elapsed(),
            byte_average_hex(&frame)
        ),
        Err(e) => println!("\nsnapshot failed: {e}"),
    }
    drop(capturer);
    let t = Instant::now();
    match capture_thumbnail(target, 640) {
        Ok(frame) => println!(
            "one-shot thumbnail: {}x{} in {:.1?} (byte mean {})",
            frame.width,
            frame.height,
            t.elapsed(),
            byte_average_hex(&frame)
        ),
        Err(e) => println!("one-shot thumbnail failed: {e}"),
    }
}
