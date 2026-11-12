# Lighting integrations

Glowdrift computes one smoothed colour. An integration is a way of getting that colour onto lights. There are
three routes. Only the first is being implemented; the other two are plans and may change.

| Route | Status | Control | Needs |
| --- | --- | --- | --- |
| 1. Corsair iCUE SDK, direct | In development | Glowdrift sets colours itself | iCUE installed and running |
| 2. Philips Hue, local bridge API | Planned | Glowdrift sets colours itself | A Hue bridge on the local network |
| 3. Virtual display | Planned, optional | Another app samples a display that Glowdrift fills | A virtual display driver you install yourself |

## 1. Corsair iCUE SDK (default)

**Implemented / in development.**

Glowdrift talks to the running iCUE application through Corsair's official iCUE SDK (version 4). The SDK is
Corsair's supported way for another program to set the colours of iCUE-managed devices, so Glowdrift sends colours
directly. No virtual display and no screen capture by iCUE is involved.

How it works in Glowdrift:

- The SDK is a native library (`iCUESDK.x64_2019.dll`) that Glowdrift loads at run time. It looks in the
  executable's folder, its `resources` subfolder, the app's bundled resource folder and an `icue-sdk` folder in the
  app data directory (`%APPDATA%\dev.glowdrift.desktop\icue-sdk`). `PATH` and the working directory are never
  searched. If the library or iCUE itself is missing, the output reports `unavailable` and the rest of the app keeps
  working.
- The output connects, enumerates devices, and applies the current colour to the devices' lighting. The status
  panel shows the state (`connected`, `connecting`, `unavailable`, `error`, `disabled`), a detail message and the
  number of devices found.
- A device that fails to report its LEDs or to accept a colour is skipped and logged while the other devices keep
  working. The status detail names the skipped devices, and they are tried again on a later enumeration with a
  growing delay (1 s up to 30 s).
- Colours are sent only when they change.
- Disabling the output, or pausing the engine, stops Glowdrift sending colours and shuts the output down.

Trade-offs:

- Works only with Corsair devices that iCUE controls.
- iCUE must be running. iCUE may require its SDK access to be enabled in its settings; check Corsair's
  documentation for your iCUE version.
- Control is shared with iCUE and its own lighting effects. How conflicts between Glowdrift and an iCUE lighting
  effect are resolved is decided by iCUE's SDK access rules, and has not been verified here.
- The SDK library is distributed by Corsair and is not included in this repository or in the installer, because
  its licence text grants no explicit redistribution right. When the library is not found, the Lights page and the
  setup step show a "Download Corsair library" button. Pressing it downloads the library on the user's machine; the
  user can also copy `redist/x64/iCUESDK.x64_2019.dll` from Corsair's `CorsairOfficial/cue-sdk` release into one of
  the folders above by hand.

### Library download

The download is pinned to release `v4.0.84` of `CorsairOfficial/cue-sdk` (`iCUESDK_4.0.84.zip`):

- HTTPS only. The request and every redirect must target `github.com` or one of GitHub's release-asset hosts; at
  most 5 redirects, a 10 s connect timeout, a 60 s total timeout and a 2 MiB size cap.
- The archive's SHA-256 is compared with a constant in the source. Only the member
  `iCUESDK/redist/x64/iCUESDK.x64_2019.dll` is read, its SHA-256 is compared with a second constant, and archive
  member names are never used as file paths.
- The library is written to `icue-sdk` in the app data directory through a temporary file and a rename. A file that
  already has the expected checksum is left alone.
- The iCUE output then retries loading at once; no restart is needed.
- Nothing is downloaded until the user presses the button.

Changing the pinned version means updating the URL and both checksums in `apps/desktop/src-tauri/src/icue_sdk.rs`.

## 2. Philips Hue through the bridge's local API

**Planned. Nothing here is implemented.**

The intent is to talk to a Hue bridge directly on the local network rather than going through Hue Sync.

- Pairing uses the bridge's local authorisation (pressing the link button), and the resulting application key is
  stored in the app's data on this PC.
- For slow changes, the bridge's ordinary lighting API can set light colours. It is rate limited, so it suits a
  calm, heavily smoothed colour, not rapid changes.
- Hue also offers an Entertainment streaming mode for low-latency, high-rate updates to lights in an
  entertainment area. It needs an area configured in the Hue app, a streaming session to the bridge, and the
  bridge only allows limited concurrent streams. Whether Glowdrift needs streaming, given its slow fades, is an
  open question.

Trade-offs:

- Local control, no cloud account involved.
- Requires the bridge and the PC to be on the same network.
- Running at the same time as Hue Sync may conflict over the same lights; the details have not been tested.

## 3. Virtual display for screen-sampling apps

**Planned and optional. Nothing here is implemented.**

Some products can only take their colours from a display: for example iCUE's screen-based lighting features or
Hue Sync. They cannot be fed a colour directly. The idea is to give them something to sample: a virtual monitor
that Glowdrift fills with the smoothed colour. The other app is pointed at that monitor and shows the calm colour
on the lights.

Constraints that shape the design:

- A virtual monitor requires a Windows indirect display driver. Glowdrift will **not** ship a custom driver. The
  plan is to use an existing, signed indirect display driver that the user installs themselves, and to have
  Glowdrift draw a full-screen colour on that display.
- Installing a driver needs administrator rights and changes the display topology. Glowdrift will not do that
  automatically; it will explain the step and detect the display once it exists.
- The other app must be able to select that display as its source, and this has not been verified for any
  specific product.
- This route adds a display to Windows (extra window management, possible effects on games and HDR), which is why
  it is optional and not the default.

Trade-offs compared with direct control:

- Works with anything that can sample a display, including devices Glowdrift has no direct integration for.
- More moving parts, driver installation, and added latency from the other app's own sampling and processing.
- The other app's own effects (its own smoothing, saturation and brightness) apply on top of Glowdrift's.

## Adding another output

Outputs implement the `Output` trait in `crates/glowdrift-outputs`:

```rust
pub trait Output: Send {
    fn id(&self) -> &str;
    fn status(&self) -> OutputStatus;
    fn set_enabled(&mut self, enabled: bool);
    fn set_color(&mut self, color: Rgb) -> Result<(), OutputError>;
    fn shutdown(&mut self);
}
```

`set_color` receives the smoothed sRGB colour, and only when it changed. `status` is polled about once a second.
New outputs also need an entry in the configuration schema (`outputs`) and the IPC `OutputStatus.kind` type; see
[architecture.md](architecture.md).
