//! Per-thread CPU meter for a running process (Windows).
//!
//! Samples kernel+user time of the matching thread(s) and of the whole process over a fixed
//! window, then prints CPU as a percentage of one core, plus the CPU model.
//!
//! Usage: `cpu_meter --process <name|pid> [--thread <name>] [--seconds <n>] [--list-threads]`
//!
//! `--thread` defaults to `glowdrift-engine`. Threads are matched by the name set with
//! `SetThreadDescription` (what `std::thread::Builder::name` sets). If several threads share the
//! name their times are summed. Threads that start or exit during the window are not counted
//! consistently, so measure against a steady process.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HLOCAL, LocalFree};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::{
    GetProcessTimes, GetThreadDescription, GetThreadTimes, OpenProcess, OpenThread,
    PROCESS_QUERY_LIMITED_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::core::w;

const DEFAULT_THREAD: &str = "glowdrift-engine";

struct Args {
    process: String,
    thread: String,
    seconds: f64,
    list_threads: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        process: String::new(),
        thread: DEFAULT_THREAD.to_owned(),
        seconds: 60.0,
        list_threads: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--process" => args.process = value()?,
            "--thread" => args.thread = value()?,
            "--seconds" => args.seconds = value()?.parse().map_err(|e| format!("{e}"))?,
            "--list-threads" => args.list_threads = true,
            "--help" | "-h" => {
                println!(
                    "Usage: cpu_meter --process <name|pid> [--thread <name>] [--seconds <n>] \
                     [--list-threads]\n\nDefaults: --thread {DEFAULT_THREAD}, --seconds 60."
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

/// Kernel + user time of a thread in 100 ns units.
fn thread_cpu(handle: HANDLE) -> Option<u64> {
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
    Some(filetime_100ns(kernel) + filetime_100ns(user))
}

/// Kernel + user time of a process in 100 ns units.
fn process_cpu(handle: HANDLE) -> Option<u64> {
    let (mut create, mut exit, mut kernel, mut user) = Default::default();
    // SAFETY: all out pointers refer to live locals.
    unsafe {
        GetProcessTimes(
            handle,
            &raw mut create,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .ok()?;
    Some(filetime_100ns(kernel) + filetime_100ns(user))
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

    let threads: Vec<(u32, Thread)> = thread_ids(pid)?
        .into_iter()
        .filter_map(|tid| open_thread(tid).map(|t| (tid, t)))
        .filter(|(_, t)| t.name == args.thread)
        .collect();
    if threads.is_empty() {
        return Err(format!(
            "no thread named {:?} in pid {pid} (try --list-threads)",
            args.thread
        ));
    }

    let start_threads: HashMap<u32, u64> = threads
        .iter()
        .filter_map(|(tid, t)| thread_cpu(t.handle).map(|c| (*tid, c)))
        .collect();
    let start_process = process_cpu(process).ok_or("GetProcessTimes failed")?;
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs_f64(args.seconds));
    let wall = start.elapsed().as_secs_f64();
    let end_process = process_cpu(process).ok_or("GetProcessTimes failed")?;

    let mut thread_delta = 0u64;
    for (tid, t) in &threads {
        match (start_threads.get(tid), thread_cpu(t.handle)) {
            (Some(a), Some(b)) => thread_delta += b.saturating_sub(*a),
            _ => eprintln!("warning: thread {tid} could not be sampled at both ends"),
        }
    }
    // SAFETY: `process` was opened above and is not used afterwards.
    let _ = unsafe { CloseHandle(process) };

    let to_pct = |delta_100ns: u64| delta_100ns as f64 * 1e-7 / wall * 100.0;
    let thread_pct = to_pct(thread_delta);
    let process_pct = to_pct(end_process.saturating_sub(start_process));
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    println!("cpu:            {}", cpu_model());
    println!("process:        pid {pid} ({})", args.process);
    println!("window:         {wall:.2} s");
    println!(
        "thread {:?} ({} found): {thread_pct:.3} % of one core ({:.3} ms CPU)",
        args.thread,
        threads.len(),
        thread_delta as f64 * 1e-4,
    );
    println!(
        "process total:  {process_pct:.3} % of one core ({:.3} % of {cores} logical cores)",
        process_pct / cores as f64
    );
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
