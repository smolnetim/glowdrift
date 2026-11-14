//! The only module that touches the iCUE SDK's C ABI.
//!
//! Struct layouts and signatures are transcribed from `iCUESDK.h` (iCUE SDK v4). The SDK DLL is
//! loaded at run time from explicit directories only, so the application starts without iCUE.

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code)]

    use std::ffi::{c_char, c_int, c_void};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};

    use glowdrift_core::Rgb;
    use libloading::os::windows::{
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library,
    };

    use crate::icue::sdk::{
        DeviceInfo, ErrorCode, EventSink, Sdk, SdkError, SdkEvent, SessionDetails, SessionState,
        Version,
    };

    /// File name of the 64-bit SDK library.
    pub(crate) const DLL_NAME: &str = "iCUESDK.x64_2019.dll";

    const STRING_SIZE_M: usize = 128;
    const DEVICE_COUNT_MAX: usize = 64;
    const DEVICE_LEDCOUNT_MAX: usize = 512;
    /// `CDT_All`: matches every device type.
    const DEVICE_TYPE_ALL: i32 = -1;

    type DeviceIdBuf = [u8; STRING_SIZE_M];

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CorsairVersion {
        major: c_int,
        minor: c_int,
        patch: c_int,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CorsairSessionDetails {
        client: CorsairVersion,
        server: CorsairVersion,
        host: CorsairVersion,
    }

    #[repr(C)]
    struct CorsairSessionStateChanged {
        state: c_int,
        details: CorsairSessionDetails,
    }

    #[repr(C)]
    struct CorsairDeviceInfo {
        device_type: c_int,
        id: DeviceIdBuf,
        serial: [u8; STRING_SIZE_M],
        model: [u8; STRING_SIZE_M],
        led_count: c_int,
        channel_count: c_int,
    }

    impl CorsairDeviceInfo {
        const ZERO: CorsairDeviceInfo = CorsairDeviceInfo {
            device_type: 0,
            id: [0; STRING_SIZE_M],
            serial: [0; STRING_SIZE_M],
            model: [0; STRING_SIZE_M],
            led_count: 0,
            channel_count: 0,
        };
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CorsairLedPosition {
        id: u32,
        cx: f64,
        cy: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CorsairLedColor {
        id: u32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    }

    #[repr(C)]
    struct CorsairDeviceFilter {
        device_type_mask: c_int,
    }

    /// `CorsairEvent`: an id followed by a union whose members are both pointers.
    #[repr(C)]
    struct CorsairEvent {
        id: c_int,
        payload: *const c_void,
    }

    const EVENT_DEVICE_CONNECTION_STATUS_CHANGED: c_int = 1;

    type SessionHandler = unsafe extern "C" fn(*mut c_void, *const CorsairSessionStateChanged);
    type EventHandler = unsafe extern "C" fn(*mut c_void, *const CorsairEvent);

    /// Resolved exports. The function pointers are valid while `_lib` is loaded; `_lib` is
    /// declared last and the whole struct is only ever owned by [`FfiSdk`].
    struct Api {
        connect: unsafe extern "C" fn(SessionHandler, *mut c_void) -> c_int,
        get_session_details: unsafe extern "C" fn(*mut CorsairSessionDetails) -> c_int,
        disconnect: unsafe extern "C" fn() -> c_int,
        get_devices: unsafe extern "C" fn(
            *const CorsairDeviceFilter,
            c_int,
            *mut CorsairDeviceInfo,
            *mut c_int,
        ) -> c_int,
        get_led_positions: unsafe extern "C" fn(
            *const c_char,
            c_int,
            *mut CorsairLedPosition,
            *mut c_int,
        ) -> c_int,
        subscribe_for_events: unsafe extern "C" fn(EventHandler, *mut c_void) -> c_int,
        unsubscribe_from_events: unsafe extern "C" fn() -> c_int,
        set_led_colors: unsafe extern "C" fn(*const c_char, c_int, *const CorsairLedColor) -> c_int,
        get_led_colors: unsafe extern "C" fn(*const c_char, c_int, *mut CorsairLedColor) -> c_int,
        set_layer_priority: unsafe extern "C" fn(u32) -> c_int,
        release_control: unsafe extern "C" fn(*const c_char) -> c_int,
        _lib: Library,
    }

    /// Shared target of the SDK callbacks.
    struct CallbackCtx {
        sink: Mutex<Option<EventSink>>,
    }

    impl CallbackCtx {
        fn emit(&self, event: SdkEvent) {
            let sink = self
                .sink
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(sink) = sink {
                sink(event);
            }
        }

        fn set_sink(&self, sink: Option<EventSink>) {
            *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = sink;
        }
    }

    /// `CorsairSessionStateChangedHandler`.
    unsafe extern "C" fn on_session_state(
        context: *mut c_void,
        data: *const CorsairSessionStateChanged,
    ) {
        // A panic must not unwind into the SDK's thread.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `context` is the pointer registered in `FfiSdk::connect`, which points
            // into an `Arc<CallbackCtx>` that `FfiSdk` keeps alive until after the library is
            // unloaded. `data` is valid for the duration of the callback.
            let (ctx, data) = unsafe { (context.cast::<CallbackCtx>().as_ref(), data.as_ref()) };
            if let (Some(ctx), Some(data)) = (ctx, data) {
                ctx.emit(SdkEvent::Session {
                    state: SessionState::from_raw(data.state),
                    details: details_from_raw(data.details),
                });
            }
        }));
    }

    /// `CorsairEventHandler`.
    unsafe extern "C" fn on_event(context: *mut c_void, event: *const CorsairEvent) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: same contract as `on_session_state`.
            let (ctx, event) = unsafe { (context.cast::<CallbackCtx>().as_ref(), event.as_ref()) };
            if let (Some(ctx), Some(event)) = (ctx, event)
                && event.id == EVENT_DEVICE_CONNECTION_STATUS_CHANGED
            {
                ctx.emit(SdkEvent::DevicesChanged);
            }
        }));
    }

    fn version_from_raw(v: CorsairVersion) -> Version {
        Version {
            major: v.major,
            minor: v.minor,
            patch: v.patch,
        }
    }

    fn details_from_raw(d: CorsairSessionDetails) -> SessionDetails {
        SessionDetails {
            client: version_from_raw(d.client),
            server: version_from_raw(d.server),
            host: version_from_raw(d.host),
        }
    }

    fn check(function: &'static str, raw: c_int) -> Result<(), SdkError> {
        if raw == 0 {
            Ok(())
        } else {
            Err(SdkError::Call {
                function,
                code: ErrorCode::from_raw(raw),
            })
        }
    }

    fn c_string(bytes: &[u8]) -> String {
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    fn device_id_buf(id: &str) -> Result<DeviceIdBuf, SdkError> {
        let bytes = id.as_bytes();
        if bytes.len() >= STRING_SIZE_M || bytes.contains(&0) {
            return Err(SdkError::Call {
                function: "device id",
                code: ErrorCode::InvalidArguments,
            });
        }
        let mut buf = [0u8; STRING_SIZE_M];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(buf)
    }

    fn buf_len(len: usize) -> Result<c_int, SdkError> {
        c_int::try_from(len).map_err(|_| SdkError::Call {
            function: "led count",
            code: ErrorCode::InvalidArguments,
        })
    }

    /// Finds the SDK DLL in `dirs` (absolute paths, tried in order) and builds `Api` from the
    /// first copy that loads and has every export. A copy that fails is remembered and the next
    /// directory is tried; the first such error is returned only when no copy works. Dependencies
    /// resolve from the System32 directory and the DLL's own directory only, never from `PATH` or
    /// the current directory.
    fn load_api(dirs: &[PathBuf]) -> Result<Api, SdkError> {
        let mut searched = Vec::new();
        let mut first_error = None;
        for dir in dirs {
            let Ok(dir) = std::path::absolute(dir) else {
                continue;
            };
            let candidate = dir.join(DLL_NAME);
            if !candidate.is_file() {
                searched.push(dir);
                continue;
            }
            match load_file(&candidate).and_then(Api::from_library) {
                Ok(api) => return Ok(api),
                Err(err) => {
                    log::warn!(
                        "iCUE SDK library at {} unusable: {err}",
                        candidate.display()
                    );
                    first_error.get_or_insert(err);
                }
            }
        }
        Err(first_error.unwrap_or(SdkError::LibraryMissing {
            file: DLL_NAME,
            searched,
        }))
    }

    fn load_file(path: &Path) -> Result<Library, SdkError> {
        // SAFETY: loading the vendor DLL runs its initialisation routine. The path is absolute
        // and the search flags restrict dependency lookup to System32 and the DLL's directory.
        unsafe {
            Library::load_with_flags(
                path,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }
        .map_err(|e| SdkError::LibraryLoad(format!("{}: {e}", path.display())))
    }

    macro_rules! export {
        ($lib:expr, $name:literal) => {
            // SAFETY: the declared function-pointer type of the destination field matches the
            // prototype of this export in `iCUESDK.h`.
            *unsafe { $lib.get(concat!($name, "\0").as_bytes()) }
                .map_err(|e| SdkError::LibraryLoad(format!("missing export {}: {e}", $name)))?
        };
    }

    impl Api {
        fn from_library(lib: Library) -> Result<Api, SdkError> {
            Ok(Api {
                connect: export!(lib, "CorsairConnect"),
                get_session_details: export!(lib, "CorsairGetSessionDetails"),
                disconnect: export!(lib, "CorsairDisconnect"),
                get_devices: export!(lib, "CorsairGetDevices"),
                get_led_positions: export!(lib, "CorsairGetLedPositions"),
                subscribe_for_events: export!(lib, "CorsairSubscribeForEvents"),
                unsubscribe_from_events: export!(lib, "CorsairUnsubscribeFromEvents"),
                set_led_colors: export!(lib, "CorsairSetLedColors"),
                get_led_colors: export!(lib, "CorsairGetLedColors"),
                set_layer_priority: export!(lib, "CorsairSetLayerPriority"),
                release_control: export!(lib, "CorsairReleaseControl"),
                _lib: lib,
            })
        }
    }

    /// [`Sdk`] backed by the real iCUE SDK DLL.
    pub(crate) struct FfiSdk {
        // Field order matters: the library (inside `api`) unloads before the callback context
        // is freed, so no SDK thread can observe a dangling context.
        api: Option<Api>,
        dirs: Vec<PathBuf>,
        ctx: Arc<CallbackCtx>,
        session_open: bool,
        events_subscribed: bool,
        led_buffer: Vec<CorsairLedColor>,
    }

    impl std::fmt::Debug for FfiSdk {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FfiSdk")
                .field("loaded", &self.api.is_some())
                .field("session_open", &self.session_open)
                .finish_non_exhaustive()
        }
    }

    impl FfiSdk {
        /// `dirs` are the only places the DLL is looked for. Nothing is loaded until
        /// [`Sdk::connect`].
        pub fn new(dirs: Vec<PathBuf>) -> FfiSdk {
            FfiSdk {
                api: None,
                dirs,
                ctx: Arc::new(CallbackCtx {
                    sink: Mutex::new(None),
                }),
                session_open: false,
                events_subscribed: false,
                led_buffer: Vec::new(),
            }
        }

        fn api(&self) -> Result<&Api, SdkError> {
            self.api.as_ref().ok_or(SdkError::Call {
                function: "iCUE SDK",
                code: ErrorCode::NotConnected,
            })
        }

        fn context(&self) -> *mut c_void {
            Arc::as_ptr(&self.ctx).cast_mut().cast()
        }
    }

    impl Sdk for FfiSdk {
        fn connect(&mut self, sink: EventSink) -> Result<(), SdkError> {
            if self.api.is_none() {
                self.api = Some(load_api(&self.dirs)?);
            }
            self.ctx.set_sink(Some(sink));
            let context = self.context();
            let connect = self.api()?.connect;
            // SAFETY: `on_session_state` matches `CorsairSessionStateChangedHandler` and
            // `context` stays valid until the library is unloaded (see `FfiSdk` field order).
            let raw = unsafe { connect(on_session_state, context) };
            if let Err(err) = check("CorsairConnect", raw) {
                self.ctx.set_sink(None);
                return Err(err);
            }
            self.session_open = true;
            Ok(())
        }

        fn disconnect(&mut self) {
            let Some(api) = self.api.as_ref() else { return };
            if self.events_subscribed {
                // SAFETY: no arguments; valid after a successful load.
                let _ = unsafe { (api.unsubscribe_from_events)() };
                self.events_subscribed = false;
            }
            if self.session_open {
                // SAFETY: no arguments; the SDK invokes the state callback with `Closed`
                // before returning, which only enqueues an event.
                let _ = unsafe { (api.disconnect)() };
                self.session_open = false;
            }
            self.ctx.set_sink(None);
        }

        fn session_details(&mut self) -> Result<SessionDetails, SdkError> {
            let mut details = CorsairSessionDetails {
                client: CorsairVersion {
                    major: 0,
                    minor: 0,
                    patch: 0,
                },
                server: CorsairVersion {
                    major: 0,
                    minor: 0,
                    patch: 0,
                },
                host: CorsairVersion {
                    major: 0,
                    minor: 0,
                    patch: 0,
                },
            };
            // SAFETY: `details` is a valid, writable `CorsairSessionDetails`.
            let raw = unsafe { (self.api()?.get_session_details)(&raw mut details) };
            check("CorsairGetSessionDetails", raw)?;
            Ok(details_from_raw(details))
        }

        fn set_layer_priority(&mut self, priority: u32) -> Result<(), SdkError> {
            // SAFETY: plain integer argument.
            let raw = unsafe { (self.api()?.set_layer_priority)(priority) };
            check("CorsairSetLayerPriority", raw)
        }

        fn subscribe_events(&mut self) -> Result<(), SdkError> {
            let context = self.context();
            let subscribe = self.api()?.subscribe_for_events;
            // SAFETY: `on_event` matches `CorsairEventHandler`; `context` outlives the library.
            let raw = unsafe { subscribe(on_event, context) };
            check("CorsairSubscribeForEvents", raw)?;
            self.events_subscribed = true;
            Ok(())
        }

        fn devices(&mut self) -> Result<Vec<DeviceInfo>, SdkError> {
            let filter = CorsairDeviceFilter {
                device_type_mask: DEVICE_TYPE_ALL,
            };
            let mut buffer: Vec<CorsairDeviceInfo> = (0..DEVICE_COUNT_MAX)
                .map(|_| CorsairDeviceInfo::ZERO)
                .collect();
            let mut size: c_int = 0;
            // SAFETY: `buffer` holds `DEVICE_COUNT_MAX` initialised elements and the SDK writes
            // at most `sizeMax` of them; `filter` and `size` are valid for the call.
            let raw = unsafe {
                (self.api()?.get_devices)(
                    &raw const filter,
                    buf_len(DEVICE_COUNT_MAX)?,
                    buffer.as_mut_ptr(),
                    &raw mut size,
                )
            };
            check("CorsairGetDevices", raw)?;
            let count = usize::try_from(size).unwrap_or(0).min(buffer.len());
            Ok(buffer[..count]
                .iter()
                .map(|d| DeviceInfo {
                    id: c_string(&d.id),
                    model: c_string(&d.model),
                    device_type: u32::try_from(d.device_type).unwrap_or(0),
                    led_count: u32::try_from(d.led_count).unwrap_or(0),
                })
                .collect())
        }

        fn led_ids(&mut self, device: &str) -> Result<Vec<u32>, SdkError> {
            let id = device_id_buf(device)?;
            let mut positions = vec![
                CorsairLedPosition {
                    id: 0,
                    cx: 0.0,
                    cy: 0.0
                };
                DEVICE_LEDCOUNT_MAX
            ];
            let mut size: c_int = 0;
            // SAFETY: `id` is NUL-terminated; `positions` has `DEVICE_LEDCOUNT_MAX` writable
            // elements; `size` is valid for the call.
            let raw = unsafe {
                (self.api()?.get_led_positions)(
                    id.as_ptr().cast(),
                    buf_len(DEVICE_LEDCOUNT_MAX)?,
                    positions.as_mut_ptr(),
                    &raw mut size,
                )
            };
            check("CorsairGetLedPositions", raw)?;
            let count = usize::try_from(size).unwrap_or(0).min(positions.len());
            Ok(positions[..count].iter().map(|p| p.id).collect())
        }

        fn set_leds_color(
            &mut self,
            device: &str,
            led_ids: &[u32],
            color: Rgb,
        ) -> Result<(), SdkError> {
            let id = device_id_buf(device)?;
            self.led_buffer.clear();
            self.led_buffer
                .extend(led_ids.iter().map(|&id| CorsairLedColor {
                    id,
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: 255,
                }));
            let size = buf_len(self.led_buffer.len())?;
            // SAFETY: `id` is NUL-terminated and `led_buffer` holds `size` initialised entries.
            let raw = unsafe {
                (self.api()?.set_led_colors)(id.as_ptr().cast(), size, self.led_buffer.as_ptr())
            };
            check("CorsairSetLedColors", raw)
        }

        fn led_colors(&mut self, device: &str, led_ids: &[u32]) -> Result<Vec<Rgb>, SdkError> {
            let id = device_id_buf(device)?;
            let mut colors: Vec<CorsairLedColor> = led_ids
                .iter()
                .map(|&id| CorsairLedColor {
                    id,
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0,
                })
                .collect();
            let size = buf_len(colors.len())?;
            // SAFETY: `id` is NUL-terminated; `colors` holds `size` writable entries whose ids
            // select the LEDs to read.
            let raw = unsafe {
                (self.api()?.get_led_colors)(id.as_ptr().cast(), size, colors.as_mut_ptr())
            };
            check("CorsairGetLedColors", raw)?;
            Ok(colors.iter().map(|c| Rgb::new(c.r, c.g, c.b)).collect())
        }

        fn release_control(&mut self, device: &str) {
            let (Ok(id), Ok(api)) = (device_id_buf(device), self.api()) else {
                return;
            };
            // SAFETY: `id` is NUL-terminated.
            let _ = unsafe { (api.release_control)(id.as_ptr().cast()) };
        }
    }

    impl Drop for FfiSdk {
        fn drop(&mut self) {
            self.disconnect();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn scratch_dir(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("glowdrift-ffi-test-{}-{tag}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn load_error(dirs: &[PathBuf]) -> SdkError {
            match load_api(dirs) {
                Ok(_) => panic!("no copy should load"),
                Err(err) => err,
            }
        }

        #[test]
        fn directories_without_the_library_are_reported_as_missing() {
            let empty = scratch_dir("empty");
            let err = load_error(std::slice::from_ref(&empty));
            std::fs::remove_dir_all(&empty).unwrap();
            assert!(matches!(err, SdkError::LibraryMissing { .. }), "{err}");
        }

        #[test]
        fn unloadable_copies_are_skipped_and_the_first_error_is_returned() {
            let first = scratch_dir("first");
            let second = scratch_dir("second");
            let empty = scratch_dir("none");
            std::fs::write(first.join(DLL_NAME), b"not a dll 1").unwrap();
            std::fs::write(second.join(DLL_NAME), b"not a dll 2").unwrap();
            let err = load_error(&[first.clone(), empty.clone(), second.clone()]);
            for dir in [&first, &second, &empty] {
                std::fs::remove_dir_all(dir).unwrap();
            }
            let SdkError::LibraryLoad(message) = err else {
                panic!("expected a load error, got {err}");
            };
            assert!(message.contains(first.to_str().unwrap()), "{message}");
            assert!(!message.contains(second.to_str().unwrap()), "{message}");
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    use glowdrift_core::Rgb;

    use crate::icue::sdk::{DeviceInfo, EventSink, Sdk, SdkError, SessionDetails};

    pub(crate) const DLL_NAME: &str = "iCUESDK.x64_2019.dll";

    fn unsupported() -> SdkError {
        SdkError::LibraryMissing {
            file: DLL_NAME,
            searched: Vec::new(),
        }
    }

    /// Placeholder for platforms without the SDK; connecting reports the library as missing.
    #[derive(Debug)]
    pub(crate) struct FfiSdk;

    impl FfiSdk {
        pub fn new(_dirs: Vec<PathBuf>) -> FfiSdk {
            FfiSdk
        }
    }

    impl Sdk for FfiSdk {
        fn connect(&mut self, _sink: EventSink) -> Result<(), SdkError> {
            Err(unsupported())
        }
        fn disconnect(&mut self) {}
        fn session_details(&mut self) -> Result<SessionDetails, SdkError> {
            Err(unsupported())
        }
        fn set_layer_priority(&mut self, _priority: u32) -> Result<(), SdkError> {
            Err(unsupported())
        }
        fn subscribe_events(&mut self) -> Result<(), SdkError> {
            Err(unsupported())
        }
        fn devices(&mut self) -> Result<Vec<DeviceInfo>, SdkError> {
            Err(unsupported())
        }
        fn led_ids(&mut self, _device: &str) -> Result<Vec<u32>, SdkError> {
            Err(unsupported())
        }
        fn set_leds_color(
            &mut self,
            _device: &str,
            _led_ids: &[u32],
            _color: Rgb,
        ) -> Result<(), SdkError> {
            Err(unsupported())
        }
        fn led_colors(&mut self, _device: &str, _led_ids: &[u32]) -> Result<Vec<Rgb>, SdkError> {
            Err(unsupported())
        }
        fn release_control(&mut self, _device: &str) {}
    }
}

pub(crate) use imp::{DLL_NAME, FfiSdk};
