# Security policy

## Supported versions

Glowdrift is pre-1.0. Security fixes are made against the latest release only.

## Reporting a vulnerability

Please report vulnerabilities privately. Do not open a public issue.

1. Go to the repository's **Security** tab and choose **Report a vulnerability** (GitHub private vulnerability
   reporting).
2. Describe the problem, the affected version, and steps to reproduce it. A proof of concept helps.

You can expect an acknowledgement within a few days. Glowdrift is maintained by volunteers, so there is no fixed
response time, but confirmed vulnerabilities are prioritised and fixed in a new release, with credit if you want
it.

## Scope

In scope:

- The Glowdrift application and its installer.
- The update mechanism, including signature verification and the release workflow.
- The IPC surface between the webview and the Rust shell.

Out of scope:

- Vulnerabilities in third-party products Glowdrift controls (Corsair iCUE, Philips Hue) that are not caused by
  how Glowdrift uses them. Report those to the vendor.
- Issues that need an attacker who already has administrator rights on the PC.

## Notes for users

- Download installers only from this repository's Releases page.
- Glowdrift reads screen content locally to compute a colour and does not store or transmit it.
- Update payloads are signed, and the app verifies the signature before applying an update.
