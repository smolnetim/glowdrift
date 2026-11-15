# Windows code signing (Authenticode)

Why the installer is not signed yet, what the options cost, and how to turn signing on. Facts checked against the
linked sources in November 2026; prices and eligibility rules change, so re-check before committing to one.

Authenticode is separate from the updater's minisign signature (see [updates.md](updates.md#signing-keys)). The
updater signature is always on; Authenticode only affects what Windows shows when a user runs the installer.

## Current state

- `release.yml` has an opt-in **Azure Artifact Signing** path. It runs only when all six secrets exist in the
  `release` environment: `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID`, `AZURE_SIGNING_ENDPOINT`,
  `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE`.
- When enabled, the workflow installs [`artifact-signing-cli`](https://github.com/Levminer/artifact-signing-cli) and
  writes a config override that sets Tauri's `bundle.windows.signCommand`. Tauri signs the app executable and the
  NSIS installer during the build, so the minisign signature is computed over the already signed installer.
- With no secrets set (today), releases are unsigned. The path is untested against a real account.

## What users see

SmartScreen looks at two things: the signing certificate's publisher reputation and the reputation of the file
hash ([Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)).

- **Unsigned:** "Windows protected your PC", the user must choose "Run anyway". Reputation starts at zero for every
  new version, because nothing carries over between unsigned files. On Windows 11, Smart App Control can block
  unsigned files that lack a positive reputation, and enterprise policy can remove "Run anyway".
- **Signed (OV, Artifact Signing, or similar):** the verified publisher name is shown, but a new publisher still
  gets the "unrecognized app" warning until downloads accumulate. There is no threshold and no way to request a
  review for consumer endpoints; Microsoft says it can take weeks and hundreds of clean installs. Certificate
  reputation can then spare later versions signed with the same trusted certificate, which unsigned files never get.
- **EV does not help:** EV certificates no longer give instant reputation, per
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) and
  [Tauri](https://v2.tauri.app/distribute/sign/windows/). Do not pay extra for EV to avoid the warning.
- Only the Microsoft Store avoids the warning entirely. It needs different packaging and is out of scope here.

## Options

| Option | Cost | Eligibility for an individual in Germany | CI fit | SmartScreen effect |
| --- | --- | --- | --- | --- |
| Azure Artifact Signing | Basic 9.99 USD/month (5,000 signatures, 0.005 USD each beyond), Premium 99.99 USD/month ([retail price API](https://prices.azure.com/api/retail/prices?$filter=contains(productName,'Signing')%20and%20priceType%20eq%20'Consumption'), [pricing page](https://azure.microsoft.com/en-us/pricing/details/artifact-signing/)). Billed in full from account creation, not prorated. | **No as an individual**: individual developers must be in the US or Canada. Organizations in the EU are eligible, which needs a legal business entity with public records ([quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart)). Whether a German sole proprietor counts as an organization is not documented. | Already wired in `release.yml`. No hardware token. | Publisher name shown; reputation builds like OV. |
| SignPath Foundation (free OSS signing) | Free | Open to OSS projects with an OSI licence; no stated country limit. Solo-maintained projects appear among applicants, but no rule is published ([terms](https://signpath.org/terms)). | Needs a workflow change (below). GitHub-hosted runners only. | Certificate belongs to SignPath Foundation, so that name is shown, not "T1M". |
| Certum Open Source Code Signing | Listed at 49 EUR on the shop ([Certum](https://shop.certum.eu/open-source-code-signing-on-simplysign.html)); the billing period is not stated there. Standard cloud certificate from 209 EUR ([Certum](https://shop.certum.eu/standard-code-signing-in-the-cloud.html)). | Yes: issued to a natural person after ID check, a utility bill and proof of a public open source project ([required documents](https://support.certum.eu/en/code-signing-required-documents/)). Subject shows "Open Source Developer". | Poor. Key lives in the SimplySign cloud and unlocks with codes from a mobile app. No official unattended CI route; community tools replay the TOTP seed, which then becomes a CI secret. 5,000 signatures/month cap. | OV-like: reputation builds over time. |
| Other OV certificate | Roughly the Certum standard price and up (not checked per vendor). | Yes, with identity checks. | Since 2023-06-01 CAs must keep the private key in a hardware crypto module ([CA/Browser Forum](https://cabforum.org/working-groups/code-signing/requirements/)), so it is a token or a vendor cloud HSM, which is awkward in CI. Validity is capped at 460 days for certificates issued from 2026-03-01. | Same as OV above. |
| EV certificate | Higher than OV. | Organizations only in practice. | Same hardware constraint. | No advantage over OV. |
| Stay unsigned | Free | n/a | n/a | Warning on every new version. |

## Enable Azure Artifact Signing

Prerequisite: Microsoft accepts the identity validation (see eligibility above). Check this before paying for
anything, because validation can fail and takes 1 to 20 business days. Source for all steps:
[quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart).

1. Use a paid Azure subscription. Free, trial and sponsored subscriptions are not supported
   ([FAQ](https://learn.microsoft.com/en-us/azure/artifact-signing/faq)). Register the `Microsoft.CodeSigning`
   resource provider on it.
2. Create an Artifact Signing account (portal, or `az artifact-signing create ... --sku Basic`). Basic is enough:
   a release signs a handful of files. The account name is 3 to 24 characters and globally unique. Pick a region and
   note its endpoint, for example `https://weu.codesigning.azure.net` for West Europe. The endpoint must match the
   account's region, or signing fails with 403.
3. Give your own user the **Artifact Signing Identity Verifier** role, then create a **Public** identity validation
   in the portal (it cannot be done in the CLI). Organization: legal name, website, a monitored email on the
   organization's domain, business identifier and address, plus a personal ID check of the representative. Fix
   mistakes by creating a new request. Identity validation is shared by all accounts in the subscription.
4. Create a **Public Trust** certificate profile bound to that validation. The certificate subject is the validated
   legal name and cannot be customized.
5. Register an app in Microsoft Entra, create a client secret, and assign the app the **Artifact Signing
   Certificate Profile Signer** role on the account or on the profile
   ([roles](https://learn.microsoft.com/en-us/azure/artifact-signing/tutorial-assign-roles)):

   ```powershell
   az role assignment create --assignee <app object id> --role "Artifact Signing Certificate Profile Signer" `
     --scope "/subscriptions/<id>/resourceGroups/<rg>/providers/Microsoft.CodeSigning/codeSigningAccounts/<account>/certificateProfiles/<profile>"
   ```

6. Add the secrets to the `release` environment (see [releasing.md](releasing.md)):

   | Secret | Value |
   | --- | --- |
   | `AZURE_TENANT_ID` | Directory (tenant) ID of the app registration |
   | `AZURE_CLIENT_ID` | Application (client) ID |
   | `AZURE_CLIENT_SECRET` | The client secret value. It expires; rotate it before then. |
   | `AZURE_SIGNING_ENDPOINT` | Region endpoint from step 2 |
   | `AZURE_SIGNING_ACCOUNT` | Account name |
   | `AZURE_SIGNING_PROFILE` | Certificate profile name |

7. Test with a pre-release tag (for example `v0.x.0-rc.1`). On the downloaded installer check the Digital
   Signatures tab or `Get-AuthenticodeSignature`: status valid, publisher as validated, and a countersignature
   timestamp. Then confirm the updater still accepts it.

Notes:

- Certificates are valid for 72 hours and renewed daily, so the timestamp is what keeps old signatures valid. The
  service's timestamp authority is `http://timestamp.acs.microsoft.com`
  ([certificate management](https://learn.microsoft.com/en-us/azure/artifact-signing/concept-certificate-management)).
  Check that the signing command applies a timestamp; the CLI's flags are in its
  [README](https://github.com/Levminer/artifact-signing-cli).
- The identity validation expires and must be renewed, or signing stops (reminders start 60 days before).
- `artifact-signing-cli` needs .NET, the Azure CLI and SignTool on the runner. Confirm that the hosted Windows
  image provides them on the first run.
- Deleting the account stops billing; the identity validation stays at subscription level.

## SignPath Foundation (if chosen)

Requirements from the [terms](https://signpath.org/terms): an OSI licence for all components, no proprietary
parts, an actively maintained project that is already released in the form to be signed, signed binaries built
automatically from the public repository, a "Code signing policy" section on the project home page naming the
roles and linking the privacy statement, multi-factor authentication for members, and manual approval of every
release. The Foundation can decline any application. Apply at [signpath.org](https://signpath.org/).

Workflow impact ([GitHub integration](https://docs.signpath.io/trusted-build-systems/github)): SignPath signs a
file you upload as a workflow artifact and returns it, so it cannot be a synchronous `signCommand`. The release
job would change to:

1. Build the installer without updater artifacts, upload the unsigned installer with `actions/upload-artifact`
   (v4 or later).
2. Submit it with `signpath/github-action-submit-signing-request` (organization ID, project and signing policy
   slugs, API token as a secret) and download the signed installer. All earlier jobs must run on GitHub-hosted
   runners.
3. Compute the minisign signature over the **signed** file with `tauri signer sign`, since signing changes the
   file, and generate `latest.json` and the release upload from the signed installer rather than through
   `tauri-action`'s own upload.
4. The executable inside the installer is only signed if it is sent through SignPath before packaging, which means
   two signing requests per release.

## Recommendation

The maintainer decides. Based on the sources above:

- **Azure Artifact Signing is the best technical fit and is already wired in, but an individual in Germany cannot
  pass its identity validation.** It becomes the right choice if the project is released by a registered business
  entity, or if Azure Support confirms in writing that a German sole proprietorship validates as an organization.
  Ask before paying; the cost is small (about 120 USD/year) but validation is the risk.
- **For an individual, SignPath Foundation is the best fit**: free, open source friendly, no hardware token, and
  it works with GitHub-hosted runners. The costs are a workflow rework, a per-release approval, the Foundation
  name as publisher, and uncertainty about acceptance of a new, single-maintainer project.
- **Certum is the fallback** if SignPath declines: individual-friendly and cheap, but unattended CI signing is not
  officially supported.
- **Do not buy EV** for SmartScreen purposes.
- Until one of these is in place, ship unsigned and note the SmartScreen prompt in the release notes or README.
  Signing does not remove the first-weeks warning for any option; it makes reputation durable across versions.
