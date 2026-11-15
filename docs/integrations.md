# Lighting integrations

Glowdrift computes one smoothed colour. An integration is a way of getting that colour onto lights. There are
three routes. The first two are implemented; the third is a plan and may change.

| Route | Status | Control | Needs |
| --- | --- | --- | --- |
| 1. Corsair iCUE SDK, direct | In development | Glowdrift sets colours itself | iCUE installed and running |
| 2. Philips Hue, local bridge API | Implemented | Glowdrift sets colours itself | A Hue bridge on the local network |
| 3. Virtual display | Planned, optional | Another app samples a display that Glowdrift fills | A virtual display driver you install yourself |

## 1. Corsair iCUE SDK (default)

**Implemented / in development.**

Glowdrift talks to the running iCUE application through Corsair's official iCUE SDK (version 4). The SDK is
Corsair's supported way for another program to set the colours of iCUE-managed devices, so Glowdrift sends colours
directly. No virtual display and no screen capture by iCUE is involved.

How it works in Glowdrift:

- The SDK is a native library (`iCUESDK.x64_2019.dll`) that Glowdrift loads at run time. It looks in the
  executable's folder, its `resources` subfolder, the app's bundled resource folder and an `icue-sdk` folder in the
  app data directory (`%APPDATA%\gg.t1m.glowdrift\icue-sdk`). `PATH` and the working directory are never
  searched. If the library or iCUE itself is missing, the light reports `unavailable` and the rest of the app keeps
  working.
- The light connects, enumerates devices, and applies the current colour to the devices' lighting. The status
  panel shows the state (`connected`, `connecting`, `unavailable`, `error`, `disabled`), a detail message and the
  number of devices found.
- A device that fails to report its LEDs or to accept a colour is skipped and logged while the other devices keep
  working. The status detail names the skipped devices, and they are tried again on a later enumeration with a
  growing delay (1 s up to 30 s).
- Colours are sent only when they change.
- Disabling the light, or pausing the engine, stops Glowdrift sending colours and shuts the light down.

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
- The iCUE light then retries loading at once; no restart is needed.
- Nothing is downloaded until the user presses the button.

Changing the pinned version means updating the URL and both checksums in `apps/desktop/src-tauri/src/icue_sdk.rs`.

## 2. Philips Hue through the bridge's local API

**Implemented.** Glowdrift talks to the bridge's local CLIP v2 REST API; no Hue account or cloud login is involved.
Entertainment streaming is deferred; see [ADR 0003](adr/0003-hue-output-rest-first.md) for the reasoning.

How it works in Glowdrift:

- **Discovery.** The Lights page's "Find bridge" browses mDNS (`_hue._tcp.local`). Only when mDNS finds nothing is
  Signify's discovery service (`discovery.meethue.com`) asked, which sends the PC's public address to the
  internet. A bridge can also be entered by address; its public `/api/0/config` is read to identify it.
- **Pairing.** The user presses Pair, then the bridge's link button. Glowdrift polls the bridge every 2 s for up to
  30 s and gives up with "The link button was not pressed in time." The bridge returns an application key and a
  client key.
- **Trust.** Bridge certificates are not signed by a public CA, so the SHA-256 of the bridge's leaf certificate is
  recorded when pairing (trust on first use). Every later connection must present exactly that certificate; a
  changed certificate is refused and the light shows an error until the user pairs again.
- **Credentials.** The address, bridge id, application key, client key and certificate pin are stored encrypted for
  the current Windows user (DPAPI) in `hue-bridge.bin` in the app data directory, never in `config.json`.
  `config.json` keeps only `lights.hue.enabled`, `bridgeId`, `areaId`, `extraLights` and `zones`. A file that
  cannot be decrypted, or one whose bridge id differs from `bridgeId`, counts as not paired. "Forget bridge" deletes the file.
- **Target.** The user picks one entertainment area (rooms and zones are not offered, because only entertainment
  areas carry light positions) and may add single bridge lights that are not in it ("Extra lights" on the Lights
  page, stored in `lights.hue.extraLights`).
- **Zones.** Each Hue light follows its own rectangle of the whole display, smoothed independently; iCUE keeps the
  whole-region colour from `region`. Zones are fractions of the monitor, so they may lie outside the iCUE region
  and may overlap. Defaults: a light in the entertainment area is centred horizontally at `(x + 1) / 2` and
  vertically at `(1 - z) / 2` from its position in the Hue app's entertainment area setup, one third of the
  display wide and two thirds tall, kept on screen; an extra light has no position and gets a centred box one
  fifth wide and full height. The Capture page shows the display with the iCUE box and one labelled box per light
  with its live colour; dragging or resizing a box stores an override in `lights.hue.zones` (light id to
  rectangle), and "Reset to Hue positions" removes the overrides, for one light or all.
- **State restore.** When Glowdrift starts sending to a light it records the light's on/off state and brightness
  and either its colour or its colour temperature, whichever it had. When it stops sending, each light is put
  back to that state. Stopping means: the Hue light is disabled, the engine is paused, the area or the extra
  lights change, the bridge is forgotten, or the app exits. A light that cannot be reached at that moment is
  skipped. A crash or power loss cannot restore anything; the lights then keep the last colour they were given.
- **Output.** For each light, with the colour of its own zone, `PUT /clip/v2/resource/light/{id}` with on/off, brightness, CIE xy colour (clamped
  into the light's gamut) and a transition. At most 10 requests per second go out in total, one light per slot in
  round-robin order, so a light is refreshed every `lights x 100 ms`. That period is also the transition time and
  the light's minimum send interval, so the bridge fades between updates. Only the latest colour is kept; nothing
  queues. Brightness is the largest 8-bit colour channel as a percentage (at least 1 %); a colour whose largest
  channel is below 3 turns the light off.
- **Status.** The light reports `notPaired` or `noArea` (status codes) until a bridge and an area or extra light are set. An
  unreachable bridge shows as `unavailable` and is retried with a growing delay (1 s up to 30 s); an area that no
  longer exists is reported as `noArea` and retried the same way. A rejected application key, a changed
  certificate or unusable stored details show as `error` and are not retried until the user pairs again.
- Disabling the light, or pausing the engine, stops the requests and restores the lights (see State restore).

Trade-offs:

- Local control, no cloud account involved. The bridge and the PC must be on the same network.
- The bridge's rate limit makes this suit calm, heavily smoothed colour; a large area refreshes each light less
  often.
- Running at the same time as Hue Sync or another app that sets the same lights may conflict; the details have not
  been tested.

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

## Adding another light

Lights implement the `Light` trait in `crates/glowdrift-lights`:

```rust
pub trait Light: Send {
    fn id(&self) -> &str;
    fn status(&self) -> LightStatus;
    fn set_enabled(&mut self, enabled: bool);
    fn set_color(&mut self, color: Rgb) -> Result<(), LightError>;
    fn shutdown(&mut self);
}
```

`set_color` receives the smoothed sRGB colour, and only when it changed. `status` is polled about once a second.
New lights also need an entry in the configuration schema (`lights`) and the IPC `LightStatus.kind` type; see
[architecture.md](architecture.md).
