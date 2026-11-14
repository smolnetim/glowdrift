//! Per-thread CPU meter for a running process (Windows).
//!
//! Measures thread and process CPU from the cycle counters (`QueryThreadCycleTime`,
//! `QueryProcessCycleTime`) over a fixed window and prints it as a percentage of one core, plus the
//! CPU model. The cycle counters are exact; `GetThreadTimes` is sampled by the scheduler tick
//! (about 15.6 ms) and under-counts a thread that wakes briefly and often by roughly 10x, so it is
//! shown only as a labelled coarse extra.
//!
//! Cycles become time with the rate that `glowdrift_capture::cpu::cycles_per_second` measures by
//! spinning this thread for a known wall time. That is unit-agnostic: whatever the counter ticks
//! in, a thread that runs flat out for the window reads 100 %. The registry `~MHz` value is printed
//! next to it only as a cross-check, because it is the nominal frequency and can differ from the
//! counter's rate.
//!
//! Usage: `cpu_meter --process <name|pid> [--thread <name>] [--seconds <n>] [--all-threads]
//! [--list-threads]`
//!
//! `--thread` defaults to `glowdrift-engine`. Threads are matched by the name set with
//! `SetThreadDescription` (what `std::thread::Builder::name` sets). If several threads share the
//! name their times are summed. `--all-threads` prints every thread of the process, busiest first.
//! Threads that exit during the window are still counted (their handles stay open); threads that
//! start during the window are counted from their start in `--all-threads` mode only.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HLOCAL, LocalFree};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::Threading::{
    GetThreadDescription, GetThreadTimes, OpenProcess, OpenThread,
    PROCESS_QUERY_LIMITED_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};
use windows::core::w;

const DEFAULT_THREAD: &str = "glowdrift-engine";

struct Args {
    process: String,
    thread: String,
    seconds: f64,
    list_threads: bool,
    all_threads: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        process: String::new(),
        thread: DEFAULT_THREAD.to_owned(),
        seconds: 60.0,
        list_threads: false,
        all_threads: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--process" => args.process = value()?,
            "--thread" => args.thread = value()?,
            "--seconds" => args.seconds = value()?.parse().map_err(|e| format!("{e}"))?,
            "--list-threads" => args.list_threads = true,
            "--all-threads" => args.all_threads = true,
            "--help" | "-h" => {
                println!(
                    "Usage: cpu_meter --process <name|pid> [--thread <name>] [--seconds <n>] \
                     [--all-threads] [--list-threads]\n\n\
                     --all-threads lists every thread, busiest first.\n\
                     Defaults: --thread {DEFAULT_THREAD}, --seconds 60."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if args.process.is_empty() {
        return Err("--process <name|pid> is required".to_owned());
    }
    if args.seconds <= 0.0 {
        return Err("--seconds must be positive".to_owned());
    }
    Ok(args)
}

fn filetime_100ns(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// Converts a NUL-terminated UTF-16 buffer to a `String`.
fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn find_pid(spec: &str) -> Result<u32, String> {
    if let Ok(pid) = spec.parse::<u32>() {
        return Ok(pid);
    }
    let wanted = spec.to_ascii_lowercase();
    let has_exe = std::path::Path::new(&wanted)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    let wanted_exe = if has_exe {
        wanted.clone()
    } else {
        format!("{wanted}.exe")
    };
    // SAFETY: plain snapshot of the process list; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| format!("process snapshot: {e}"))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut matches = Vec::new();
    // SAFETY: `entry.dwSize` is set and `snap` is a live snapshot handle.
    let mut ok = unsafe { Process32FirstW(snap, &raw mut entry) }.is_ok();
    while ok {
        let name = wide_to_string(&entry.szExeFile).to_ascii_lowercase();
        if name == wanted || name == wanted_exe {
            matches.push(entry.th32ProcessID);
        }
        // SAFETY: same as above.
        ok = unsafe { Process32NextW(snap, &raw mut entry) }.is_ok();
    }
    // SAFETY: `snap` was created above and is not used afterwards.
    let _ = unsafe { CloseHandle(snap) };
    match matches.as_slice() {
        [] => Err(format!("no process named {spec}")),
        [pid] => Ok(*pid),
        many => Err(format!(
            "{} processes named {spec} (pids {many:?}); pass a PID",
            many.len()
        )),
    }
}

/// Thread ids of `pid`.
fn thread_ids(pid: u32) -> Result<Vec<u32>, String> {
    // SAFETY: plain snapshot of all threads; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
        .map_err(|e| format!("thread snapshot: {e}"))?;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut tids = Vec::new();
    // SAFETY: `entry.dwSize` is set and `snap` is a live snapshot handle.
    let mut ok = unsafe { Thread32First(snap, &raw mut entry) }.is_ok();
    while ok {
        if entry.th32OwnerProcessID == pid {
            tids.push(entry.th32ThreadID);
        }
        // SAFETY: same as above.
        ok = unsafe { Thread32Next(snap, &raw mut entry) }.is_ok();
    }
    // SAFETY: `snap` was created above and is not used afterwards.
    let _ = unsafe { CloseHandle(snap) };
    Ok(tids)
}

struct Thread {
    handle: HANDLE,
    name: String,
}

impl Drop for Thread {
    fn drop(&mut self) {
        // SAFETY: the handle came from `OpenThread` and is owned by this value.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

fn open_thread(tid: u32) -> Option<Thread> {
    // SAFETY: plain open call; failure (exited thread, access denied) is handled.
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) }.ok()?;
    // SAFETY: `handle` is valid. On success the returned buffer is system-allocated.
    let name = match unsafe { GetThreadDescription(handle) } {
        Ok(desc) => {
            // SAFETY: `desc` is a NUL-terminated wide string until freed below.
            let s = unsafe { desc.to_string() }.unwrap_or_default();
            // SAFETY: the description buffer must be released with `LocalFree`.
            unsafe { LocalFree(Some(HLOCAL(desc.0.cast()))) };
            s
        }
        Err(_) => String::new(),
    };
    Some(Thread { handle, name })
}

/// Cycles a thread has run, from the cycle counter.
fn thread_cycles(handle: HANDLE) -> Option<u64> {
    let mut cycles = 0u64;
    // SAFETY: `handle` is a live thread handle and the out pointer refers to a live local.
    unsafe { QueryThreadCycleTime(handle, &raw mut cycles) }.ok()?;
    Some(cycles)
}

/// Cycles every thread of a process has run, including exited ones.
fn process_cycles(handle: HANDLE) -> Option<u64> {
    let mut cycles = 0u64;
    // SAFETY: `handle` is a live process handle and the out pointer refers to a live local.
    unsafe { QueryProcessCycleTime(handle, &raw mut cycles) }.ok()?;
    Some(cycles)
}

/// Kernel and user time of a thread in 100 ns units, as sampled by the scheduler tick.
fn thread_coarse_times(handle: HANDLE) -> Option<(u64, u64)> {
    let (mut create, mut exit, mut kernel, mut user) = Default::default();
    // SAFETY: all out pointers refer to live locals.
    unsafe {
        GetThreadTimes(
            handle,
            &raw mut create,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .ok()?;
    Some((filetime_100ns(kernel), filetime_100ns(user)))
}

fn cpu_model() -> String {
    let mut buf = [0u16; 256];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: `buf` and `bytes` are live locals and `bytes` is the buffer size in bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    };
    if status.is_ok() {
        wide_to_string(&buf).trim().to_owned()
    } else {
        format!("unknown (registry error {})", status.0)
    }
}

/// Nominal processor frequency in MHz from the registry, for comparison with the measured rate.
fn nominal_mhz() -> Option<u32> {
    let mut mhz = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: `mhz` and `bytes` are live locals and `bytes` is the buffer size in bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("~MHz"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut mhz).cast()),
            Some(&raw mut bytes),
        )
    };
    status.is_ok().then_some(mhz)
}

/// Cycles one thread ran over the window.
struct ThreadUse {
    tid: u32,
    name: String,
    cycles: u64,
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let pid = find_pid(&args.process)?;

    if args.list_threads {
        let mut names: Vec<(u32, String)> = thread_ids(pid)?
            .into_iter()
            .filter_map(|tid| open_thread(tid).map(|t| (tid, t.name.clone())))
            .collect();
        names.sort();
        for (tid, name) in names {
            println!(
                "{tid}\t{}",
                if name.is_empty() { "<unnamed>" } else { &name }
            );
        }
        return Ok(());
    }

    // SAFETY: plain open call; failure is reported.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|e| format!("open process {pid}: {e}"))?;

    let tracked: Vec<(u32, Thread)> = thread_ids(pid)?
        .into_iter()
        .filter_map(|tid| open_thread(tid).map(|t| (tid, t)))
        .filter(|(_, t)| args.all_threads || t.name == args.thread)
        .collect();
    if tracked.is_empty() {
        return Err(format!(
            "no thread named {:?} in pid {pid} (try --list-threads)",
            args.thread
        ));
    }

    let rate = glowdrift_capture::cpu::cycles_per_second();
    let start_cycles: HashMap<u32, u64> = tracked
        .iter()
        .filter_map(|(tid, t)| thread_cycles(t.handle).map(|c| (*tid, c)))
        .collect();
    let start_coarse: HashMap<u32, (u64, u64)> = tracked
        .iter()
        .filter_map(|(tid, t)| thread_coarse_times(t.handle).map(|c| (*tid, c)))
        .collect();
    let start_process = process_cycles(process).ok_or("QueryProcessCycleTime failed")?;
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs_f64(args.seconds));
    let wall = start.elapsed().as_secs_f64();
    let end_process = process_cycles(process).ok_or("QueryProcessCycleTime failed")?;

    let mut used: Vec<ThreadUse> = Vec::new();
    let mut coarse = (0u64, 0u64);
    for (tid, t) in &tracked {
        match (start_cycles.get(tid), thread_cycles(t.handle)) {
            (Some(a), Some(b)) => used.push(ThreadUse {
                tid: *tid,
                name: t.name.clone(),
                cycles: b.saturating_sub(*a),
            }),
            _ => eprintln!("warning: thread {tid} could not be sampled at both ends"),
        }
        if let (Some(a), Some(b)) = (start_coarse.get(tid), thread_coarse_times(t.handle)) {
            coarse.0 += b.0.saturating_sub(a.0);
            coarse.1 += b.1.saturating_sub(a.1);
        }
    }
    let mut born = 0usize;
    if args.all_threads {
        for tid in thread_ids(pid)? {
            if tracked.iter().any(|(known, _)| *known == tid) {
                continue;
            }
            if let Some(t) = open_thread(tid)
                && let Some(cycles) = thread_cycles(t.handle)
            {
                born += 1;
                used.push(ThreadUse {
                    tid,
                    name: t.name.clone(),
                    cycles,
                });
            }
        }
    }
    // SAFETY: `process` was opened above and is not used afterwards.
    let _ = unsafe { CloseHandle(process) };

    report(
        &args,
        pid,
        Sample {
            rate,
            wall,
            used,
            born,
            coarse,
            process_delta: end_process.saturating_sub(start_process),
        },
    );
    Ok(())
}

/// What one measurement window produced.
struct Sample {
    rate: f64,
    wall: f64,
    used: Vec<ThreadUse>,
    /// Threads that started during the window (`--all-threads` only).
    born: usize,
    /// Kernel and user 100 ns units of the matched threads from `GetThreadTimes`.
    coarse: (u64, u64),
    process_delta: u64,
}

fn report(args: &Args, pid: u32, sample: Sample) {
    let Sample {
        rate,
        wall,
        mut used,
        born,
        coarse,
        process_delta,
    } = sample;
    let to_pct = |cycles: u64| cycles as f64 / rate / wall * 100.0;
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    println!("cpu:            {}", cpu_model());
    let nominal =
        nominal_mhz().map_or_else(String::new, |mhz| format!(" (registry nominal {mhz} MHz)"));
    println!(
        "cycle rate:     {:.3} GHz, measured by spinning this thread{nominal}",
        rate / 1e9
    );
    println!("process:        pid {pid} ({})", args.process);
    println!("window:         {wall:.2} s");

    if args.all_threads {
        used.sort_by(|a, b| b.cycles.cmp(&a.cycles).then(a.tid.cmp(&b.tid)));
        let sum: u64 = used.iter().map(|t| t.cycles).sum();
        println!("threads (% of one core, busiest first):");
        let mut idle = 0usize;
        for t in &used {
            if t.cycles == 0 {
                idle += 1;
                continue;
            }
            let name = if t.name.is_empty() {
                "<unnamed>"
            } else {
                &t.name
            };
            println!("  {:>8.3} %  {name} (tid {})", to_pct(t.cycles), t.tid);
        }
        println!(
            "  {idle} of {} threads used no CPU; {born} started during the window",
            used.len()
        );
        println!(
            "sum of threads: {:.3} % of one core ({:.3} % of the process total is not in a listed thread)",
            to_pct(sum),
            to_pct(process_delta) - to_pct(sum),
        );
    } else {
        let delta: u64 = used.iter().map(|t| t.cycles).sum();
        println!(
            "thread {:?} ({} found): {:.3} % of one core ({:.3} ms CPU)",
            args.thread,
            used.len(),
            to_pct(delta),
            delta as f64 / rate * 1e3,
        );
        println!(
            "  coarse GetThreadTimes (tick-sampled, under-counts short bursts): user {:.3} % kernel {:.3} %",
            coarse.1 as f64 * 1e-7 / wall * 100.0,
            coarse.0 as f64 * 1e-7 / wall * 100.0,
        );
    }
    println!(
        "process total:  {:.3} % of one core ({:.3} % of {cores} logical cores)",
        to_pct(process_delta),
        to_pct(process_delta) / cores as f64
    );
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
