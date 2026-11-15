# Drive Hue lights over CLIP v2 REST first, defer Entertainment streaming

Status: accepted.

## Context

Glowdrift's output is slow, heavily smoothed colour changes. A Hue bridge can be driven two ways:

- **CLIP v2 REST.** `PUT /clip/v2/resource/light/{id}` with `dynamics.duration`, so the bridge interpolates between
  updates. The bridge accepts roughly 10 light commands per second in total.
- **Entertainment streaming.** `PUT .../entertainment_configuration/{id}` with `{"action":"start"}`, then HueStream v2
  datagrams over DTLS 1.2 on UDP 2100 at up to ~25-60 per second, then `{"action":"stop"}`.

A spike checked whether streaming is reachable from pure Rust with no OpenSSL install and no system changes, and
whether it is worth it for this workload.

## Findings

Protocol, as implemented by open-source clients and a bridge emulator (the Hue developer pages are behind a login and
were not read directly):

- DTLS 1.2, cipher `TLS_PSK_WITH_AES_128_GCM_SHA256` (0x00A8), UDP port 2100.
- PSK identity is the `hue-application-id` response header of `GET /auth/v1` on current firmware (older firmware: the
  application key). PSK is the hex-decoded `clientkey` from pairing (`generateclientkey`).
- HueStream v2 datagram: `"HueStream"`, version `02 00`, sequence byte, two reserved bytes, colour space (`00` RGB /
  `01` xy), reserved byte, 36-byte ASCII entertainment configuration id, then 7 bytes per channel (channel id,
  three big-endian u16). Up to 20 channels.
- The area must be `start`ed before the handshake is accepted; the bridge drops the stream after about 10 s without
  data; one streaming session per bridge, so it excludes other entertainment apps while active.

Crates, for a DTLS 1.2 PSK **client** with that exact suite:

| Crate | Result |
| --- | --- |
| `dtls` 0.17.3 (successor of `webrtc-dtls` 0.12.0, same suite code) | Implements `TLS_PSK_WITH_AES_128_GCM_SHA256`. Pure Rust (RustCrypto, `ring`). Async (tokio). MIT/Apache-2.0. |
| `dimpl` 0.9.0 | DTLS 1.2/1.3, PSK only with `AES_128_CCM_8`. Not usable. |
| `openssl` | Works (used by existing Hue streaming tools) but needs an OpenSSL install or vendored C build. Excluded. |

- **Loopback result.** A throwaway binary (outside the workspace) ran a DTLS-PSK server and a `dtls` client over
  UDP loopback, with a relay in between that parses the handshake. ClientHello offered `[0x00a8]`, ServerHello
  selected `0x00a8` with version `0xfefd` (DTLS 1.2), the server saw the PSK identity, and three HueStream v2
  datagrams arrived intact. Handshake took ~2 ms. Cipher negotiation and framing are therefore proven; interop with a
  real bridge is not.
- **Weight.** The `dtls` closure is 147 crates on Windows, 80 of them new to this workspace's lockfile (RustCrypto
  AEAD/curve stack, `rkyv`, `x509-parser`, `rcgen`, `webrtc-util`, `nom`), plus an async runtime that the app does
  not otherwise use for the lights path. Several duplicate versions (`rand`, `hmac`) would add `multiple-versions`
  warnings.
- **Licences.** All 147 are covered by `deny.toml` (MIT, Apache-2.0, ISC, BSD-3-Clause, Unicode-3.0; `Unlicense OR
  MIT` resolves to MIT). Checked by hand from crate metadata; `cargo deny` was not run, so advisories are unchecked.

Not verified against a real bridge:

- That the bridge accepts this client's ClientHello (extension set, extended master secret, cookie exchange) and the
  PSK identity choice.
- Real handshake time, stream timeout behaviour and recovery after a dropped session or sleep/wake.
- Visible quality of a slow fade over the stream versus REST with matched transitions.

## Decision

Ship Hue output over CLIP v2 REST only, at no more than ~10 light PUTs per second in total, with the transition
duration equal to the send interval. Do not add DTLS streaming now.

Reasons:

- Glowdrift's changes are slow and smoothed. With `dynamics.duration` equal to the interval, the bridge already
  interpolates between updates, so the extra update rate of streaming has little visible effect.
- REST needs only an app key and the user's normal Hue rooms or zones. Streaming additionally needs an entertainment
  area made in the Hue app, the client key, exclusive use of the bridge's single stream, a keep-alive, and
  reconnect handling.
- Streaming costs ~80 new crates and an unverified interop path, for no demonstrated gain on this workload.

## Considered options

- **Streaming first via `dtls`.** Feasible and proven on loopback, rejected for cost against benefit above.
- **Streaming via OpenSSL.** Violates the no-system-install requirement.
- **Both, selectable.** Doubles the surface (area selection, two failure modes) before any need is shown.

## Consequences

- Credentials still include the client key from pairing, so streaming can be added later without re-pairing.
- Revisit streaming if any of these hold: updates faster than ~1 per second per light are wanted (reactive or
  audio-driven effects); more than ~10 lights must stay in sync; REST shows visible stepping or the bridge returns
  rate-limit errors despite matched transitions; or a real-bridge run shows a clean handshake and a clearly smoother
  result.
- A later streaming backend would sit behind the existing light interface, using `dtls` with only the PSK suite
  enabled, and would first re-run the interop checks listed above.
