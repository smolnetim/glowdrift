# Profiling the engine thread

Two tools, both reading the thread cycle counter (`QueryThreadCycleTime`):

- `cpu_meter` example: CPU of a running process and its threads, from outside. Use it for totals,
  for comparing builds, and to see non-engine threads (iCUE worker and SDK threads).
- `engine-timing` feature of `glowdrift-desktop`: per-stage wall time and CPU of the engine loop,
  from inside. Compiled out by default, so normal builds carry no cost.

Do not trust `GetThreadTimes` (Task Manager's per-thread views, older meters): it is sampled by the
scheduler tick, about 15.6 ms, and under-counts a thread that wakes briefly and often by roughly
10x (the engine thread reads 0.03-0.27 % where the cycle counter says about 0.5 %).
Cycles become time with a rate measured by spinning the meter's own thread for a known wall time,
best of several rounds, so a thread running flat out reads 100 %. The measured rate is within about
1.5 % of the TSC and the registry `~MHz`; the registry value is only the nominal frequency, so it
is not used. Record the CPU model next to every number; the meter prints it.

## Meter a running app

```sh
cargo run --release -p glowdrift-capture --example cpu_meter -- --process glowdrift-desktop --seconds 60
cargo run --release -p glowdrift-capture --example cpu_meter -- --process glowdrift-desktop --seconds 60 --all-threads
```

`--process` takes a name or PID (give a PID when several instances run). The default thread is
`glowdrift-engine`; `--thread <name>` picks another and `--list-threads` shows the names. Output is
percent of one core for the thread and for the whole process, plus a `GetThreadTimes` line labelled
coarse. `--all-threads` lists every thread busiest first (unnamed ones are SDK or runtime threads)
and shows how much of the process total the listed threads account for. Use 60 s windows; 10 s is
only a smoke check.

## Enable

```sh
cargo build --profile profiling --features engine-timing -p glowdrift-desktop
```

Use `--release` when no `profiling` profile is defined. Run the resulting `glowdrift-desktop.exe`
(or `cargo run` with the same flags). With a running installed copy, the single-instance guard
forwards a second launch to it, so quit that one first.

`GLOWDRIFT_ENGINE_TIMING_SECS` sets the summary interval in seconds (default 10), read once when
the engine starts.

## Output

One info-level line per interval, and a `final` line over the whole run when the engine stops. They
go to the normal log file, `glowdrift.log` in the app's log directory
(`%LOCALAPPDATA%\<identifier>\logs`, `gg.t1m.glowdrift` for the shipped app). Debug builds also
print to stdout.

```
engine timing window 10.0s: passes=… frames=… ticks=… wakes=… busy=…% cpu=…% (in pass …%) cpu/frame=…us | stage wall_us calls/s share% cpu_us | resend … | grab … | grab.acquire … | … | sleep mean (asked mean) calls/s
```

- `passes`: loop passes; `frames`: frames delivered by the capturer; `ticks`: smoother ticks;
  `wakes`: sleeps.
- `busy`: wall time inside a pass as a share of one core's wall time. `cpu`: the engine thread's
  CPU (cycle counter) as a share of one core, `in pass` the part of it inside a pass; `cpu/frame`:
  `cpu` per delivered frame. The user/kernel split is not available from cycle counts.
- The cycle rate is measured once at start (about 0.3 s of spinning on the engine thread) and
  logged in the `engine timing on` line.
- Each stage shows `wall_us` (mean wall duration per call in microseconds), calls per second, its
  share of total pass time, and `cpu_us` (mean CPU cycles per call, as microseconds; `-` where not
  measured). `wall_us` includes waits and `cpu_us` does not, so `grab.acquire` and `grab.wait_gpu`
  show a large wall time and a small CPU time. CPU is measured for `pass`, `grab`, `reduce` and the
  `grab.*` parts only; the other stages are as short as the cost of reading the counter.
- Stages: `resend`, `grab` (split into `grab.acquire`, `grab.store`, `grab.submit`, `grab.map`,
  `grab.collect` (ready-render bookkeeping), `grab.wait_gpu` (fence sleep) and `grab.read_back`
  (pixel copy), all from `Capturer::last_timings()`, and the remainder `grab.other`), `reduce`,
  `target` (brightness, saturation, sRGB), `tick` (smoother update), `send` (lights `set_color`),
  `publish` (shared status colours), `error` (`set_error`), `preview` (`events.color`), `meter`
  (FPS meter and `report`), `other` (rest of the pass), and `sleep` (actual wall time, with the
  requested time in parentheses).
- `grab.*` call counts equal `grab` calls, including grabs that returned no new frame.

Each timed span costs two clock reads, and each CPU-timed span two counter reads (a system call
each), so the instrumented build runs slightly hotter than a normal one. The `cpu-timing` feature
of `glowdrift-capture`, enabled by `engine-timing`, adds the counter reads inside the capturer.
`probe` run with `--features cpu-timing` prints the same per-phase CPU for the capturer alone.

## Profiling the iCUE light

The `icue_load` example drives the real iCUE light with a colour that changes on every update, so
the light's cost can be measured without capture:

```sh
cargo build --profile profiling -p glowdrift-lights --example icue_load
icue_load.exe --dll-dir <dir with iCUESDK.x64_2019.dll> --secs 60 --hz 60
```

It prints its process id once connected. Read per-thread CPU with `cpu_meter --all-threads` (the
worker is the thread named `glowdrift-icue`; the SDK's own threads are unnamed). The worker writes at most about 30 times per second
whatever `--hz` is.

The SDK's cost per write grows with the number of LEDs (about 1.2 us per LED on the calling
thread, plus about 0.9 us per LED on SDK threads), so compare runs on the same set of devices.
