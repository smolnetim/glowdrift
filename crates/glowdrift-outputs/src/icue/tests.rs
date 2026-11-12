//! Worker-logic tests against a fake SDK.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;

use super::IcueOutput;
use super::controller::{
    Controller, ENUMERATE_DEBOUNCE, LAYER_PRIORITY, MIN_SEND_INTERVAL, SEND_RETRY,
};
use super::probe::probe_with;
use super::sdk::{
    DeviceInfo, ErrorCode, EventSink, Sdk, SdkError, SdkEvent, SessionDetails, SessionState,
};
use crate::{Output, OutputCode, OutputState};

#[derive(Default)]
struct FakeState {
    sink: Option<EventSink>,
    connect_error: Option<SdkError>,
    devices: Vec<(DeviceInfo, Vec<u32>)>,
    devices_error: Option<SdkError>,
    set_error: Option<(String, ErrorCode)>,
    led_ids_error: Option<(String, ErrorCode)>,
    connects: u32,
    disconnects: u32,
    enumerations: u32,
    releases: Vec<String>,
    priorities: Vec<u32>,
    writes: Vec<(String, usize, Rgb)>,
}

#[derive(Clone, Default)]
struct FakeSdk(Arc<Mutex<FakeState>>);

impl FakeSdk {
    fn with_devices(devices: &[(&str, u32)]) -> FakeSdk {
        let fake = FakeSdk::default();
        fake.0.lock().unwrap().devices = devices
            .iter()
            .map(|(id, leds)| {
                (
                    DeviceInfo {
                        id: (*id).to_owned(),
                        model: (*id).to_owned(),
                        device_type: 1,
                        led_count: *leds,
                    },
                    (1..=*leds).collect(),
                )
            })
            .collect();
        fake
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.0.lock().unwrap()
    }

    fn write_count(&self) -> usize {
        self.state().writes.len()
    }
}

impl Sdk for FakeSdk {
    fn connect(&mut self, sink: EventSink) -> Result<(), SdkError> {
        let mut s = self.state();
        s.connects += 1;
        if let Some(err) = s.connect_error.clone() {
            return Err(err);
        }
        s.sink = Some(sink);
        Ok(())
    }
    fn disconnect(&mut self) {
        let mut s = self.state();
        s.disconnects += 1;
        s.sink = None;
    }
    fn session_details(&mut self) -> Result<SessionDetails, SdkError> {
        Ok(SessionDetails::default())
    }
    fn set_layer_priority(&mut self, priority: u32) -> Result<(), SdkError> {
        self.state().priorities.push(priority);
        Ok(())
    }
    fn subscribe_events(&mut self) -> Result<(), SdkError> {
        Ok(())
    }
    fn devices(&mut self) -> Result<Vec<DeviceInfo>, SdkError> {
        let mut s = self.state();
        s.enumerations += 1;
        if let Some(err) = s.devices_error.clone() {
            return Err(err);
        }
        Ok(s.devices.iter().map(|(d, _)| d.clone()).collect())
    }
    fn led_ids(&mut self, device: &str) -> Result<Vec<u32>, SdkError> {
        if let Some((id, code)) = self.state().led_ids_error.clone()
            && id == device
        {
            return Err(SdkError::Call {
                function: "CorsairGetLedPositions",
                code,
            });
        }
        self.state()
            .devices
            .iter()
            .find(|(d, _)| d.id == device)
            .map(|(_, ids)| ids.clone())
            .ok_or(SdkError::Call {
                function: "CorsairGetLedPositions",
                code: ErrorCode::DeviceNotFound,
            })
    }
    fn set_leds_color(
        &mut self,
        device: &str,
        led_ids: &[u32],
        color: Rgb,
    ) -> Result<(), SdkError> {
        let mut s = self.state();
        if let Some((id, code)) = s.set_error.clone()
            && id == device
        {
            return Err(SdkError::Call {
                function: "CorsairSetLedColors",
                code,
            });
        }
        s.writes.push((device.to_owned(), led_ids.len(), color));
        Ok(())
    }
    fn led_colors(&mut self, _device: &str, led_ids: &[u32]) -> Result<Vec<Rgb>, SdkError> {
        Ok(vec![Rgb::new(9, 9, 9); led_ids.len()])
    }
    fn release_control(&mut self, device: &str) {
        self.state().releases.push(device.to_owned());
    }
}

fn session(state: SessionState) -> SdkEvent {
    SdkEvent::Session {
        state,
        details: SessionDetails::default(),
    }
}

fn noop_sink() -> EventSink {
    Arc::new(|_| {})
}

struct Harness {
    ctl: Controller<FakeSdk>,
    fake: FakeSdk,
    t0: Instant,
}

impl Harness {
    fn new(fake: FakeSdk) -> Harness {
        Harness {
            ctl: Controller::new(fake.clone(), noop_sink()),
            fake,
            t0: Instant::now(),
        }
    }

    fn at(&self, ms: u64) -> Instant {
        self.t0 + Duration::from_millis(ms)
    }

    /// Enables, connects and reaches `Connected` with devices enumerated.
    fn connected(fake: FakeSdk) -> Harness {
        let mut h = Harness::new(fake);
        h.ctl.set_enabled(true);
        h.ctl.tick(h.at(0));
        h.ctl.on_event(session(SessionState::Connecting), h.at(0));
        h.ctl.on_event(session(SessionState::Connected), h.at(1));
        h.ctl.tick(h.at(1));
        h
    }
}

#[test]
fn starts_disabled_and_idle() {
    let mut h = Harness::new(FakeSdk::default());
    assert_eq!(h.ctl.status().state, OutputState::Disabled);
    assert_eq!(h.ctl.tick(h.at(0)), None);
    assert_eq!(h.fake.state().connects, 0);
}

#[test]
fn missing_library_is_unavailable_and_retries_with_backoff() {
    let fake = FakeSdk::default();
    fake.state().connect_error = Some(SdkError::LibraryMissing {
        file: "iCUESDK.x64_2019.dll",
        searched: vec![],
    });
    let mut h = Harness::new(fake);
    h.ctl.set_enabled(true);

    let next = h.ctl.tick(h.at(0));
    assert_eq!(h.ctl.status().state, OutputState::Unavailable);
    assert!(
        h.ctl
            .status()
            .detail
            .as_deref()
            .unwrap()
            .contains("not found")
    );
    assert_eq!(next, Some(h.at(1_000)));

    // Not due yet: no new attempt.
    assert_eq!(h.ctl.tick(h.at(500)), Some(h.at(1_000)));
    assert_eq!(h.fake.state().connects, 1);

    assert_eq!(h.ctl.tick(h.at(1_000)), Some(h.at(3_000)));
    assert_eq!(h.ctl.tick(h.at(3_000)), Some(h.at(7_000)));
    assert_eq!(h.fake.state().connects, 3);
}

#[test]
fn missing_library_sets_a_machine_readable_code_that_clears_after_retry_now() {
    let fake = FakeSdk::default();
    fake.state().connect_error = Some(SdkError::LibraryMissing {
        file: "iCUESDK.x64_2019.dll",
        searched: vec![],
    });
    let mut h = Harness::new(fake);
    h.ctl.set_enabled(true);
    assert_eq!(h.ctl.tick(h.at(0)), Some(h.at(1_000)));
    assert_eq!(h.ctl.status().code, Some(OutputCode::LibraryMissing));

    // The library appears while the controller is waiting out its back-off.
    h.fake.state().connect_error = None;
    assert_eq!(h.ctl.tick(h.at(100)), Some(h.at(1_000)));
    h.ctl.retry_now();
    h.ctl.tick(h.at(100));
    assert_eq!(h.fake.state().connects, 2);
    assert_eq!(h.ctl.status().code, None);
    assert_eq!(h.ctl.status().state, OutputState::Connecting);
}

#[test]
fn retry_now_does_nothing_while_disabled_or_connected() {
    let fake = FakeSdk::default();
    let mut h = Harness::new(fake);
    h.ctl.retry_now();
    assert_eq!(h.ctl.tick(h.at(0)), None);
    assert_eq!(h.fake.state().connects, 0);

    h.ctl.set_enabled(true);
    h.ctl.tick(h.at(0));
    h.ctl.retry_now();
    h.ctl.tick(h.at(10));
    assert_eq!(h.fake.state().connects, 1);
}

#[test]
fn a_connect_failure_reason_is_logged_once_until_it_changes_or_a_connect_succeeds() {
    let fake = FakeSdk::default();
    let missing = || SdkError::LibraryMissing {
        file: "iCUESDK.x64_2019.dll",
        searched: vec![],
    };
    fake.state().connect_error = Some(missing());
    let mut h = Harness::new(fake);
    h.ctl.set_enabled(true);

    let mut now = h.at(0);
    for _ in 0..5 {
        now = h.ctl.tick(now).unwrap();
    }
    assert_eq!(h.fake.state().connects, 5);
    assert_eq!(h.ctl.connect_failures_logged, 1);

    // A different reason is logged again, then stays quiet.
    h.fake.state().connect_error = Some(SdkError::LibraryLoad("boom".to_owned()));
    for _ in 0..3 {
        now = h.ctl.tick(now).unwrap();
    }
    assert_eq!(h.ctl.connect_failures_logged, 2);

    // After a successful connect the same failure is reported again.
    h.fake.state().connect_error = None;
    h.ctl.tick(now);
    assert_eq!(h.ctl.status().state, OutputState::Connecting);
    h.ctl.set_enabled(false);
    h.ctl.set_enabled(true);
    h.fake.state().connect_error = Some(SdkError::LibraryLoad("boom".to_owned()));
    let mut now = h.at(1_000_000);
    for _ in 0..3 {
        now = h.ctl.tick(now).unwrap();
    }
    assert_eq!(h.ctl.connect_failures_logged, 3);
}

#[test]
fn backoff_is_capped_and_resets_after_a_successful_connect() {
    let fake = FakeSdk::default();
    fake.state().connect_error = Some(SdkError::LibraryLoad("boom".to_owned()));
    let mut h = Harness::new(fake);
    h.ctl.set_enabled(true);

    let mut now = h.at(0);
    let mut delays = Vec::new();
    for _ in 0..8 {
        let next = h.ctl.tick(now).unwrap();
        delays.push(next - now);
        now = next;
    }
    assert_eq!(h.ctl.status().state, OutputState::Error);
    assert_eq!(delays.last(), Some(&Duration::from_secs(30)));
    assert!(delays.windows(2).all(|w| w[0] <= w[1]));

    h.fake.state().connect_error = None;
    h.ctl.tick(now + Duration::from_secs(30));
    assert_eq!(h.ctl.status().state, OutputState::Connecting);
}

#[test]
fn session_states_map_to_status() {
    let mut h = Harness::new(FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]));
    h.ctl.set_enabled(true);
    h.ctl.tick(h.at(0));
    assert_eq!(h.ctl.status().state, OutputState::Connecting);

    h.ctl.on_event(session(SessionState::Timeout), h.at(1));
    assert_eq!(h.ctl.status().state, OutputState::Unavailable);
    assert_eq!(
        h.ctl.status().detail.as_deref(),
        Some("iCUE is not running")
    );

    h.ctl
        .on_event(session(SessionState::ConnectionRefused), h.at(2));
    assert_eq!(h.ctl.status().state, OutputState::Unavailable);
    assert!(
        h.ctl
            .status()
            .detail
            .as_deref()
            .unwrap()
            .contains("enable the iCUE SDK")
    );

    h.ctl.on_event(session(SessionState::Connected), h.at(3));
    assert_eq!(
        h.ctl.status().state,
        OutputState::Connecting,
        "devices not read yet"
    );
    h.ctl.tick(h.at(3));
    assert_eq!(h.ctl.status().state, OutputState::Connected);
    assert_eq!(h.ctl.status().detail.as_deref(), Some("2 devices, 5 LEDs"));
    assert_eq!(h.ctl.status().device_count, 2);
    assert_eq!(h.fake.state().priorities, vec![LAYER_PRIORITY]);

    h.ctl
        .on_event(session(SessionState::ConnectionLost), h.at(4));
    assert_eq!(h.ctl.status().state, OutputState::Connecting);
    assert_eq!(h.ctl.status().device_count, 0);
}

#[test]
fn identical_colours_are_written_once() {
    let mut h = Harness::connected(FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]));
    h.ctl.set_color(Rgb::new(10, 20, 30));
    h.ctl.tick(h.at(100));
    assert_eq!(h.fake.write_count(), 2, "one write per device");

    h.ctl.set_color(Rgb::new(10, 20, 30));
    assert_eq!(h.ctl.tick(h.at(200)), None);
    assert_eq!(h.fake.write_count(), 2);

    h.ctl.set_color(Rgb::new(11, 20, 30));
    h.ctl.tick(h.at(300));
    assert_eq!(h.fake.write_count(), 4);
    let writes = h.fake.state().writes.clone();
    assert_eq!(writes[0], ("kb".to_owned(), 3, Rgb::new(10, 20, 30)));
    assert_eq!(writes[1], ("mouse".to_owned(), 2, Rgb::new(10, 20, 30)));
}

#[test]
fn writes_are_rate_limited_and_coalesced() {
    let mut h = Harness::connected(FakeSdk::with_devices(&[("kb", 3)]));
    h.ctl.set_color(Rgb::new(1, 0, 0));
    h.ctl.tick(h.at(100));
    assert_eq!(h.fake.write_count(), 1);

    // Two changes inside the minimum interval: nothing is written yet.
    h.ctl.set_color(Rgb::new(2, 0, 0));
    let due = h.at(100) + MIN_SEND_INTERVAL;
    assert_eq!(h.ctl.tick(h.at(110)), Some(due));
    h.ctl.set_color(Rgb::new(3, 0, 0));
    assert_eq!(h.ctl.tick(h.at(120)), Some(due));
    assert_eq!(h.fake.write_count(), 1);

    // When due, only the latest colour is written.
    assert_eq!(h.ctl.tick(due), None);
    let writes = h.fake.state().writes.clone();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[1].2, Rgb::new(3, 0, 0));
}

#[test]
fn colour_set_before_connection_is_sent_once_devices_are_known() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    let mut h = Harness::new(fake);
    h.ctl.set_color(Rgb::new(5, 6, 7));
    h.ctl.set_enabled(true);
    h.ctl.tick(h.at(0));
    assert_eq!(h.fake.write_count(), 0);

    h.ctl.on_event(session(SessionState::Connected), h.at(10));
    h.ctl.tick(h.at(10));
    assert_eq!(h.fake.write_count(), 1);
    assert_eq!(h.fake.state().writes[0].2, Rgb::new(5, 6, 7));
}

#[test]
fn device_events_are_debounced_into_one_enumeration() {
    let mut h = Harness::connected(FakeSdk::with_devices(&[("kb", 3)]));
    let base = h.fake.state().enumerations;

    for ms in [100, 110, 120, 200] {
        h.ctl.on_event(SdkEvent::DevicesChanged, h.at(ms));
    }
    let due = h.at(100) + ENUMERATE_DEBOUNCE;
    assert_eq!(h.ctl.tick(h.at(150)), Some(due));
    assert_eq!(h.fake.state().enumerations, base);

    h.ctl.tick(due);
    assert_eq!(h.fake.state().enumerations, base + 1);
    assert_eq!(h.ctl.tick(due + Duration::from_secs(1)), None);
}

#[test]
fn hot_plugged_device_is_picked_up_and_receives_the_current_colour() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    let mut h = Harness::connected(fake.clone());
    h.ctl.set_color(Rgb::new(8, 8, 8));
    h.ctl.tick(h.at(100));
    assert_eq!(fake.write_count(), 1);

    fake.state().devices.push((
        DeviceInfo {
            id: "fans".to_owned(),
            model: "fans".to_owned(),
            device_type: 0x20,
            led_count: 4,
        },
        vec![1, 2, 3, 4],
    ));
    h.ctl.on_event(SdkEvent::DevicesChanged, h.at(200));
    let due = h.at(200) + ENUMERATE_DEBOUNCE;
    h.ctl.tick(due);
    assert_eq!(h.ctl.status().device_count, 2);
    let writes = fake.state().writes.clone();
    assert!(
        writes
            .iter()
            .any(|w| w.0 == "fans" && w.2 == Rgb::new(8, 8, 8))
    );
}

#[test]
fn vanished_device_triggers_re_enumeration_without_error() {
    let fake = FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]);
    let mut h = Harness::connected(fake.clone());
    fake.state().set_error = Some(("mouse".to_owned(), ErrorCode::DeviceNotFound));
    h.ctl.set_color(Rgb::new(1, 1, 1));
    let next = h.ctl.tick(h.at(100));
    assert_eq!(next, Some(h.at(100) + ENUMERATE_DEBOUNCE));
    assert_eq!(h.ctl.status().state, OutputState::Connected);

    fake.state().devices.retain(|(d, _)| d.id != "mouse");
    h.ctl.tick(h.at(100) + ENUMERATE_DEBOUNCE);
    assert_eq!(h.ctl.status().device_count, 1);
}

#[test]
fn lost_connection_during_write_waits_for_session_events() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    let mut h = Harness::connected(fake.clone());
    fake.state().set_error = Some(("kb".to_owned(), ErrorCode::NotConnected));
    h.ctl.set_color(Rgb::new(1, 1, 1));
    assert_eq!(h.ctl.tick(h.at(100)), None);
    assert_eq!(h.ctl.status().state, OutputState::Connecting);

    fake.state().set_error = None;
    h.ctl.on_event(session(SessionState::Connected), h.at(500));
    h.ctl.tick(h.at(500));
    assert_eq!(h.ctl.status().state, OutputState::Connected);
    assert_eq!(fake.write_count(), 1, "colour resent after reconnect");
}

#[test]
fn exclusive_control_by_another_client_is_reported_and_retried() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    let mut h = Harness::connected(fake.clone());
    fake.state().set_error = Some(("kb".to_owned(), ErrorCode::NoControl));
    h.ctl.set_color(Rgb::new(1, 1, 1));
    let next = h.ctl.tick(h.at(100));
    assert_eq!(h.ctl.status().state, OutputState::Error);
    assert_eq!(next, Some(h.at(100) + SEND_RETRY));

    fake.state().set_error = None;
    h.ctl.tick(h.at(100) + SEND_RETRY);
    assert_eq!(h.ctl.status().state, OutputState::Connected);
    assert_eq!(fake.write_count(), 1);
}

#[test]
fn enumeration_failure_is_an_error_that_recovers() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    fake.state().devices_error = Some(SdkError::Call {
        function: "CorsairGetDevices",
        code: ErrorCode::InvalidArguments,
    });
    let mut h = Harness::new(fake.clone());
    h.ctl.set_enabled(true);
    h.ctl.tick(h.at(0));
    h.ctl.on_event(session(SessionState::Connected), h.at(1));
    let next = h.ctl.tick(h.at(1));
    assert_eq!(h.ctl.status().state, OutputState::Error);
    assert_eq!(next, Some(h.at(1) + Duration::from_secs(1)));

    fake.state().devices_error = None;
    h.ctl.tick(h.at(1_001));
    assert_eq!(h.ctl.status().state, OutputState::Connected);
}

#[test]
fn device_that_fails_enumeration_is_skipped_and_retried_later() {
    let fake = FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]);
    fake.state().led_ids_error = Some(("mouse".to_owned(), ErrorCode::InvalidArguments));
    let mut h = Harness::connected(fake.clone());
    assert_eq!(h.ctl.status().state, OutputState::Connected);
    assert_eq!(h.ctl.status().device_count, 1);
    let detail = h.ctl.status().detail.clone().unwrap();
    assert!(detail.contains("skipped mouse"), "{detail}");

    h.ctl.set_color(Rgb::new(1, 2, 3));
    let next = h.ctl.tick(h.at(100));
    assert_eq!(fake.write_count(), 1, "only the healthy device is written");
    assert_eq!(fake.state().writes[0].0, "kb");
    assert_eq!(next, Some(h.at(1) + Duration::from_secs(1)));

    fake.state().led_ids_error = None;
    h.ctl.tick(h.at(1) + Duration::from_secs(1));
    assert_eq!(h.ctl.status().device_count, 2);
    assert!(!h.ctl.status().detail.clone().unwrap().contains("skipped"));
    assert!(fake.state().writes.iter().any(|w| w.0 == "mouse"));
}

#[test]
fn skipped_device_retries_back_off() {
    let fake = FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]);
    fake.state().led_ids_error = Some(("mouse".to_owned(), ErrorCode::InvalidArguments));
    let mut h = Harness::connected(fake);
    let first = h.at(1) + Duration::from_secs(1);
    assert_eq!(h.ctl.tick(h.at(2)), Some(first));
    let next = h.ctl.tick(first);
    assert_eq!(next, Some(first + Duration::from_secs(2)));
}

#[test]
fn no_usable_device_is_an_error_that_names_the_skipped_ones() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    fake.state().led_ids_error = Some(("kb".to_owned(), ErrorCode::InvalidArguments));
    let h = Harness::connected(fake);
    assert_eq!(h.ctl.status().state, OutputState::Error);
    assert!(h.ctl.status().detail.clone().unwrap().contains("kb"));
}

#[test]
fn device_that_rejects_colours_is_skipped_while_the_others_keep_working() {
    let fake = FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]);
    let mut h = Harness::connected(fake.clone());
    fake.state().set_error = Some(("mouse".to_owned(), ErrorCode::InvalidArguments));
    h.ctl.set_color(Rgb::new(1, 1, 1));
    let next = h.ctl.tick(h.at(100));
    assert_eq!(h.ctl.status().state, OutputState::Connected);
    assert_eq!(h.ctl.status().device_count, 1);
    assert!(
        h.ctl
            .status()
            .detail
            .clone()
            .unwrap()
            .contains("skipped mouse")
    );
    assert_eq!(fake.write_count(), 1);
    assert_eq!(next, Some(h.at(100) + Duration::from_secs(1)));

    // The same colour is not rewritten while waiting.
    assert_eq!(h.ctl.tick(h.at(200)), next);
    assert_eq!(fake.write_count(), 1);

    fake.state().set_error = None;
    h.ctl.tick(h.at(100) + Duration::from_secs(1));
    assert_eq!(h.ctl.status().device_count, 2);
    assert!(
        fake.state()
            .writes
            .iter()
            .any(|w| w.0 == "mouse" && w.2 == Rgb::new(1, 1, 1))
    );
}

#[test]
fn exclusive_control_of_one_device_does_not_stop_the_others() {
    let fake = FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]);
    let mut h = Harness::connected(fake.clone());
    fake.state().set_error = Some(("kb".to_owned(), ErrorCode::NoControl));
    h.ctl.set_color(Rgb::new(1, 1, 1));
    h.ctl.tick(h.at(100));
    assert_eq!(h.ctl.status().state, OutputState::Connected);
    assert_eq!(fake.state().writes.len(), 1);
    assert_eq!(fake.state().writes[0].0, "mouse");
}

#[test]
fn enumeration_fault_does_not_outlive_the_session() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    fake.state().devices_error = Some(SdkError::Call {
        function: "CorsairGetDevices",
        code: ErrorCode::InvalidArguments,
    });
    let mut h = Harness::new(fake);
    h.ctl.set_enabled(true);
    h.ctl.tick(h.at(0));
    h.ctl.on_event(session(SessionState::Connected), h.at(1));
    h.ctl.tick(h.at(1));
    assert_eq!(h.ctl.status().state, OutputState::Error);

    h.ctl.on_event(session(SessionState::Timeout), h.at(2));
    assert_eq!(h.ctl.status().state, OutputState::Unavailable);
    assert_eq!(
        h.ctl.status().detail.as_deref(),
        Some("iCUE is not running")
    );
}

#[test]
fn disabling_releases_control_and_disconnects_then_reenabling_resends() {
    let fake = FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]);
    let mut h = Harness::connected(fake.clone());
    h.ctl.set_color(Rgb::new(4, 4, 4));
    h.ctl.tick(h.at(100));
    assert_eq!(fake.write_count(), 2);

    h.ctl.set_enabled(false);
    assert_eq!(h.ctl.status().state, OutputState::Disabled);
    assert_eq!(
        fake.state().releases,
        vec!["kb".to_owned(), "mouse".to_owned()]
    );
    assert_eq!(fake.state().disconnects, 1);
    assert_eq!(h.ctl.tick(h.at(200)), None);

    // Colours arriving while disabled are remembered but not written.
    h.ctl.set_color(Rgb::new(5, 5, 5));
    h.ctl.tick(h.at(300));
    assert_eq!(fake.write_count(), 2);

    h.ctl.set_enabled(true);
    h.ctl.tick(h.at(400));
    assert_eq!(fake.state().connects, 2);
    h.ctl.on_event(session(SessionState::Connected), h.at(410));
    h.ctl.tick(h.at(410));
    assert_eq!(fake.write_count(), 4);
    assert_eq!(fake.state().writes[3].2, Rgb::new(5, 5, 5));
}

#[test]
fn shutdown_releases_and_stops_for_good() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    let mut h = Harness::connected(fake.clone());
    h.ctl.shutdown();
    assert_eq!(fake.state().releases, vec!["kb".to_owned()]);
    assert_eq!(fake.state().disconnects, 1);

    h.ctl.set_enabled(false);
    h.ctl.set_enabled(true);
    assert_eq!(h.ctl.tick(h.at(10)), None);
    assert_eq!(fake.state().connects, 1);
}

#[test]
fn stale_events_after_disconnect_are_ignored() {
    let mut h = Harness::new(FakeSdk::with_devices(&[("kb", 3)]));
    h.ctl.set_enabled(true);
    h.ctl.tick(h.at(0));
    h.ctl.set_enabled(false);
    h.ctl.on_event(session(SessionState::Connected), h.at(5));
    assert_eq!(h.ctl.status().state, OutputState::Disabled);
}

#[test]
fn probe_reports_devices_and_refusal() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    let sink_fake = fake.clone();
    // Deliver `Connecting` then `Connected` from a helper thread once the probe has connected.
    let feeder = std::thread::spawn(move || {
        for _ in 0..200 {
            let sink = sink_fake.state().sink.clone();
            if let Some(sink) = sink {
                sink(session(SessionState::Connecting));
                sink(session(SessionState::Connected));
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    let report = probe_with(fake, Duration::from_secs(5)).unwrap();
    feeder.join().unwrap();
    assert_eq!(report.session_states, ["Connecting", "Connected"]);
    assert_eq!(report.devices.len(), 1);
    assert_eq!(report.devices[0].led_count, 3);
    assert_eq!(report.devices[0].first_led_color, Some(Rgb::new(9, 9, 9)));

    let refused = FakeSdk::default();
    let sink_fake = refused.clone();
    let feeder = std::thread::spawn(move || {
        for _ in 0..200 {
            let sink = sink_fake.state().sink.clone();
            if let Some(sink) = sink {
                sink(session(SessionState::ConnectionRefused));
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    let err = probe_with(refused, Duration::from_millis(300)).unwrap_err();
    feeder.join().unwrap();
    assert!(err.to_string().contains("enable the iCUE SDK"), "{err}");
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn threaded_output_connects_writes_and_releases() {
    let fake = FakeSdk::with_devices(&[("kb", 3), ("mouse", 2)]);
    let mut out = IcueOutput::spawn(fake.clone());
    assert!(out.status().enabled);

    out.set_color(Rgb::new(10, 20, 30)).unwrap();
    wait_until("connect", || fake.state().sink.is_some());
    let sink = fake.state().sink.clone().unwrap();
    sink(session(SessionState::Connected));
    wait_until("first writes", || fake.write_count() == 2);
    assert_eq!(out.status().state, OutputState::Connected);
    assert_eq!(out.status().device_count, 2);
    assert_eq!(out.status().detail.as_deref(), Some("2 devices, 5 LEDs"));

    // Repeating the same colour does not wake the worker or write again.
    out.set_color(Rgb::new(10, 20, 30)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(fake.write_count(), 2);

    out.set_color(Rgb::new(1, 2, 3)).unwrap();
    wait_until("second colour", || fake.write_count() == 4);

    out.set_enabled(false);
    assert_eq!(out.status().state, OutputState::Disabled);
    wait_until("release", || fake.state().disconnects == 1);
    assert_eq!(fake.state().releases.len(), 2);

    out.shutdown();
    out.shutdown();
}

#[test]
fn dropping_the_output_stops_the_worker_and_releases_control() {
    let fake = FakeSdk::with_devices(&[("kb", 3)]);
    let out = IcueOutput::spawn(fake.clone());
    wait_until("connect", || fake.state().sink.is_some());
    let sink = fake.state().sink.clone().unwrap();
    sink(session(SessionState::Connected));
    wait_until("enumeration", || fake.state().enumerations == 1);
    drop(out);
    // Drop joins the worker, so teardown has already happened.
    assert_eq!(fake.state().disconnects, 1);
}

/// Live check against a running iCUE: writes a colour and reads it back from the hardware.
/// Run with `GLOWDRIFT_ICUE_DLL_DIR=<dir> cargo test -p glowdrift-outputs -- --ignored live`.
#[test]
#[ignore = "needs iCUE running with the SDK enabled and the SDK DLL"]
fn live_colour_round_trip() {
    use super::ffi::FfiSdk;

    let dir = std::env::var_os("GLOWDRIFT_ICUE_DLL_DIR").expect("GLOWDRIFT_ICUE_DLL_DIR");
    let mut sdk = FfiSdk::new(vec![dir.into()]);
    let (tx, rx) = std::sync::mpsc::channel();
    sdk.connect(Arc::new(move |event| {
        let _ = tx.send(event);
    }))
    .unwrap();
    loop {
        let event = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("session event");
        if matches!(
            event,
            SdkEvent::Session {
                state: SessionState::Connected,
                ..
            }
        ) {
            break;
        }
    }
    sdk.set_layer_priority(LAYER_PRIORITY).unwrap();
    let devices = sdk.devices().unwrap();
    assert_ne!(devices.len(), 0, "iCUE reported no devices");
    let target = Rgb::new(200, 0, 120);
    let mut checked = 0;
    for info in &devices {
        let ids = sdk.led_ids(&info.id).unwrap();
        sdk.set_leds_color(&info.id, &ids, target).unwrap();
    }
    std::thread::sleep(Duration::from_millis(400));
    for info in &devices {
        let ids = sdk.led_ids(&info.id).unwrap();
        let read = sdk.led_colors(&info.id, &ids).unwrap();
        let matching = read.iter().filter(|c| **c == target).count();
        println!(
            "{}: {matching}/{} LEDs read back as {}",
            info.model,
            ids.len(),
            target.to_hex()
        );
        checked += matching;
    }
    for info in &devices {
        sdk.release_control(&info.id);
    }
    sdk.disconnect();
    assert!(
        checked > 0,
        "no LED reflected the colour written through the SDK"
    );
}
