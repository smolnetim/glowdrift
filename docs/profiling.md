# Profiling the engine thread

The `engine-timing` Cargo feature of `glowdrift-desktop` times each stage of the engine loop and
reads the engine thread's CPU time. It is compiled out by default, so normal builds carry no cost.

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
engine timing window 10.0s: passes=… frames=… ticks=… wakes=… busy=…% cpu=…% (user …% kernel …%) cpu/frame=…us | stage mean_us calls/s share% | resend … | grab … | grab.acquire … | … | sleep mean (asked mean) calls/s
```

- `passes`: loop passes; `frames`: frames delivered by the capturer; `ticks`: smoother ticks;
  `wakes`: sleeps.
- `busy`: wall time inside a pass as a share of one core's wall time. `cpu`: the engine thread's
  CPU time (`GetThreadTimes`) as a share of one core; `cpu/frame`: that CPU time per delivered frame.
- `GetThreadTimes` is sampled by the scheduler in roughly 15.6 ms steps, so `cpu` is coarse over
  short windows and reads 0 when the thread uses little CPU. Compare the `final` line, or raise the
  interval, before trusting it.
- Each stage shows the mean duration per call in microseconds, calls per second, and its share of
  total pass time. Stages are wall-clock, so waits count: `grab.acquire` and `grab.map` include
  blocking, which is not CPU.
- Stages: `resend`, `grab` (split into `grab.acquire`, `grab.store`, `grab.submit`, `grab.map`
  from `Capturer::last_timings()` and the remainder `grab.other`, which is readback), `reduce`,
  `target` (brightness, saturation, sRGB), `tick` (smoother update), `send` (lights `set_color`),
  `publish` (shared status colours), `error` (`set_error`), `preview` (`events.color`), `meter`
  (FPS meter and `report`), `other` (rest of the pass), and `sleep` (actual wall time, with the
  requested time in parentheses).
- `grab.*` call counts equal `grab` calls, including grabs that returned no new frame.

Each timed span costs two clock reads, so the instrumented build runs slightly hotter than a
normal one.

## Profiling the iCUE light

The `icue_load` example drives the real iCUE light with a colour that changes on every update, so
the light's cost can be measured without capture:

```sh
cargo build --profile profiling -p glowdrift-lights --example icue_load
icue_load.exe --dll-dir <dir with iCUESDK.x64_2019.dll> --secs 60 --hz 60
```

It prints its process id once connected. Read per-thread CPU with `QueryThreadCycleTime` (the
worker is the thread named `glowdrift-icue`; the SDK's own threads are unnamed), not
`GetThreadTimes`. The worker writes at most about 30 times per second whatever `--hz` is.

The SDK's cost per write grows with the number of LEDs (about 1.2 us per LED on the calling
thread, plus about 0.9 us per LED on SDK threads), so compare runs on the same set of devices.
