//! Dev-only deterministic test pattern: a borderless window that fills one monitor and changes
//! every pixel on every frame, for repeatable capture-pipeline load measurements.
//!
//! Usage: `test_pattern [--list] [--monitor <index>] [--rate <hz>] [--seconds <n>]`
//!
//! Photosensitivity: the pattern is a slowly drifting low-saturation hue gradient. Consecutive
//! frames differ by a small, fixed hue step (1.5 degrees), never by a bright/dark flip, so there
//! is no strobing. The rate is capped at 30 Hz. Press Esc, or wait for `--seconds`, to close.
//!
//! The pattern is rendered at 256x144 and stretched to the monitor, so every screen pixel changes
//! each frame while the per-frame CPU cost stays independent of monitor resolution.

#![allow(clippy::cast_possible_wrap, clippy::too_many_lines)]

use std::time::{Duration, Instant};

use glowdrift_capture::{MonitorInfo, list_monitors};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, COLORONCOLOR, DIB_RGB_COLORS, GetDC, ReleaseDC, SRCCOPY,
    SetStretchBltMode, StretchDIBits,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
    PostQuitMessage, RegisterClassW, SW_SHOW, SetForegroundWindow, ShowWindow, TranslateMessage,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_QUIT, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};
use windows::core::w;

const VK_ESCAPE: usize = 0x1B;
const GRID_W: usize = 256;
const GRID_H: usize = 144;
const MAX_RATE_HZ: f32 = 30.0;
const HUE_STEP_DEG: f32 = 1.5;
const SATURATION: f32 = 0.45;
const VALUE: f32 = 0.65;

struct Args {
    list: bool,
    monitor: Option<usize>,
    rate: f32,
    seconds: f32,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        list: false,
        monitor: None,
        rate: 20.0,
        seconds: 60.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--list" => args.list = true,
            "--monitor" => args.monitor = Some(value()?.parse().map_err(|e| format!("{e}"))?),
            "--rate" => args.rate = value()?.parse().map_err(|e| format!("{e}"))?,
            "--seconds" => args.seconds = value()?.parse().map_err(|e| format!("{e}"))?,
            "--help" | "-h" => {
                println!(
                    "Usage: test_pattern [--list] [--monitor <index>] [--rate <hz>] [--seconds <n>]\n\n\
                     Fills one monitor with a slowly drifting, low-contrast hue gradient whose every\n\
                     pixel changes every frame. Dev-only measurement tool, not part of the app.\n\n\
                     --monitor  index from --list (default: first non-primary monitor, else 0)\n\
                     --rate     frames per second, default 20, capped at {MAX_RATE_HZ} Hz\n\
                     --seconds  run time, default 60; Esc also closes\n\n\
                     Photosensitivity: the pattern never strobes. Frames differ by a fixed 1.5 degree\n\
                     hue step at low saturation, and the rate is capped at {MAX_RATE_HZ} Hz."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if !args.rate.is_finite() || args.rate <= 0.0 {
        return Err("--rate must be positive".to_owned());
    }
    if args.rate > MAX_RATE_HZ {
        eprintln!("note: --rate capped at {MAX_RATE_HZ} Hz");
        args.rate = MAX_RATE_HZ;
    }
    if !args.seconds.is_finite() || args.seconds <= 0.0 {
        return Err("--seconds must be positive".to_owned());
    }
    Ok(args)
}

fn hsv_to_bgra(hue_deg: f32, s: f32, v: f32) -> [u8; 4] {
    let h = hue_deg.rem_euclid(360.0) / 60.0;
    let sector = h.floor();
    let f = h - sector;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match sector as u32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let to8 = |c: f32| (c * 255.0 + 0.5) as u8;
    [to8(b), to8(g), to8(r), 255]
}

/// Renders frame `n`: hue drifts by `HUE_STEP_DEG` per frame, with a fixed diagonal gradient.
fn render(buf: &mut [u8], frame: u64) {
    let base = (frame as f32 * HUE_STEP_DEG) % 360.0;
    for y in 0..GRID_H {
        for x in 0..GRID_W {
            let spread = 40.0 * x as f32 / GRID_W as f32 + 20.0 * y as f32 / GRID_H as f32;
            let px = hsv_to_bgra(base + spread, SATURATION, VALUE);
            let i = (y * GRID_W + x) * 4;
            buf[i..i + 4].copy_from_slice(&px);
        }
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_KEYDOWN if wparam.0 == VK_ESCAPE => {
            // SAFETY: `hwnd` is this window, called on its own thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: posts a quit message to this thread's queue.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: forwards the unchanged window-procedure arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn create_window(m: &MonitorInfo) -> Result<HWND, String> {
    // SAFETY: plain module-handle query for the current process.
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|e| format!("module handle: {e}"))?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: w!("GlowdriftTestPattern"),
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its pointers are 'static.
    if unsafe { RegisterClassW(&raw const class) } == 0 {
        return Err("RegisterClassW failed".to_owned());
    }
    // SAFETY: the class was registered above; all other arguments are plain values.
    unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("GlowdriftTestPattern"),
            w!("Glowdrift test pattern"),
            WS_POPUP,
            m.x,
            m.y,
            m.width as i32,
            m.height as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|e| format!("CreateWindowExW: {e}"))
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let monitors = list_monitors().map_err(|e| format!("list monitors: {e}"))?;
    if args.list {
        for (i, m) in monitors.iter().enumerate() {
            println!(
                "{i}: {}x{} at ({}, {}){}  {}",
                m.width,
                m.height,
                m.x,
                m.y,
                if m.primary { " [primary]" } else { "" },
                m.name
            );
        }
        return Ok(());
    }
    let index = args
        .monitor
        .unwrap_or_else(|| monitors.iter().position(|m| !m.primary).unwrap_or(0));
    let monitor = monitors
        .get(index)
        .ok_or_else(|| format!("no monitor {index}; use --list"))?;

    // SAFETY: called once at startup before any window exists. Failure (already set) is harmless.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let hwnd = create_window(monitor)?;
    // SAFETY: `hwnd` is a live window owned by this thread.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }

    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: GRID_W as i32,
        biHeight: -(GRID_H as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut buf = vec![0u8; GRID_W * GRID_H * 4];
    let period = Duration::from_secs_f32(1.0 / args.rate);
    let start = Instant::now();
    let end = start + Duration::from_secs_f32(args.seconds);
    let mut next = start;
    let mut frame = 0u64;
    println!(
        "test pattern on monitor {index} ({}x{}) at {:.1} Hz for {:.1} s",
        monitor.width, monitor.height, args.rate, args.seconds
    );

    'outer: loop {
        let mut msg = MSG::default();
        // SAFETY: `msg` is a live local; messages are for windows on this thread.
        while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                break 'outer;
            }
            // SAFETY: `msg` was filled in by `PeekMessageW`.
            unsafe {
                let _ = TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        }
        let now = Instant::now();
        if now >= end {
            // SAFETY: `hwnd` is a live window owned by this thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            continue;
        }
        if now < next {
            std::thread::sleep((next - now).min(Duration::from_millis(2)));
            continue;
        }
        next += period;
        if next < now {
            next = now + period;
        }
        render(&mut buf, frame);
        frame += 1;
        // SAFETY: the DC is released below; `buf` and `info` describe a GRID_W x GRID_H
        // top-down 32-bit DIB and outlive the call.
        unsafe {
            let hdc = GetDC(Some(hwnd));
            SetStretchBltMode(hdc, COLORONCOLOR);
            StretchDIBits(
                hdc,
                0,
                0,
                monitor.width as i32,
                monitor.height as i32,
                0,
                0,
                GRID_W as i32,
                GRID_H as i32,
                Some(buf.as_ptr().cast()),
                &raw const info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            ReleaseDC(Some(hwnd), hdc);
        }
    }
    let secs = start.elapsed().as_secs_f32();
    println!(
        "{frame} frames in {secs:.2} s ({:.2} Hz)",
        frame as f32 / secs
    );
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
