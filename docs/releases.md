# Desktop releases and distribution

GitHub Releases in **zevem/neptune** are the canonical desktop release store.
One intentional `v<SemVer>` tag is one release. Merges to `main` never release.
The workflow ends with a complete **draft**; a human reviews and publishes it.
**AI agents must not create/push release tags or create/publish a Neptune release
unless the user explicitly asks them to release that version.** Preparing code,
versions, changelog entries and local validation is allowed within an implementation task.

## Release-candidate policy

Every stable release, including the first public version and hotfixes, must go
through release candidates. Choose the intended stable version once, then test
`X.Y.Z-rc.1`, `X.Y.Z-rc.2`, and so on until acceptance passes. Acceptance fixes
increment the candidate number, keeping `X.Y.Z` fixed; they do not consume stable
patch or minor versions. Betas may precede RCs, but do not replace RC acceptance.

Each tagged RC is a separate build with its own dated changelog section,
installers, signed manifest and evidence. Keep its GitHub release a private draft
for owner acceptance. A green workflow, valid signatures or a complete draft
does not establish native acceptance. Record the RC tag, source SHA, installer
hashes, host details, results for every applicable
[release gate](verification.md#remaining-production-release-gates), and any
failures and retests. Stable preparation requires an accepted RC for that same
`X.Y.Z`, with evidence linked from the preparation PR.

If acceptance fails, keep the draft private, fix the blockers through the normal
feature-branch/PR process, prepare `X.Y.Z-rc.N+1`, and test its actual installers.
Preserve every existing tag, including rejected RCs. A transient CI/upload failure
may be rerun for the same tagged source before publication; changed source or
packaging needs a new RC. The helper validates versions and artifacts; it does
not enforce the acceptance decision. Agents must check the recorded evidence.

After RC acceptance, follow [stable promotion](#promote-an-accepted-release-candidate).
An RC remains a prerelease even after acceptance: promotion creates a separate
stable build, rather than renaming its tag or clearing its prerelease flag.
Creating tags/releases and publishing either channel still require explicit
authorization for that version.

### First-public-release status: 2026-10-02

Before any public release, the owner explicitly requested a reset of the private
release history: withdraw the old GitHub releases and all existing tags, then
build and publish **0.1.0-rc.1** as Neptune's first public preview. The intended
stable version is now **0.1.0**. The withdrawn private preparations are preserved
in Git history and local release evidence; they are not public stable releases.
This owner-authorized cleanup is a one-time exception to preserving tags, not a
change to the policy for subsequent releases.

The public preview remains a GitHub **prerelease**, is never marked latest, and
is offered through the Beta channel. Publication is explicitly authorized; full
native acceptance remains pending and must not be reported as passed. Stable
preparation still requires an accepted **0.1.0-rc.N**. Acceptance fixes increment
only `N`, keeping `0.1.0` fixed.

The earlier private acceptance failures informed the current source: the macOS
runtime icon uses the padded macOS asset, and Linux AppImage instructions explain
browser-download execute permissions. macOS Dock sizing needs native retesting.
The Ubuntu host's Gear Lever startup failure was separately resolved by
installing the Flatpak NVIDIA extension matching the host driver; it was not a
Neptune AppImage defect. Installer and platform checks must use the new RC's
actual bytes. Higher-version private installations require a manual install of
this reset candidate because the updater intentionally refuses downgrades.

## Prepare a release candidate

Release helpers require Python 3.11+; native release jobs provision Python 3.13.
Manifest signing uses OpenSSL's Ed25519 support on the final Linux runner.

Use `v0.1.0-rc.1`, `v0.1.0-rc.2` for acceptance candidates and `v0.1.0` for
their eventual stable release. SemVer is strict:
no leading zeroes in core or numeric prerelease identifiers. Build metadata is
accepted but does not affect precedence and must not be used to offer an update.
Core numbers must fit Windows' 16-bit VERSIONINFO fields.

The desktop version belongs to `[package]` in root `Cargo.toml` and its entry in
`Cargo.lock`. The two internal crates and private website package have independent
versions; do not bump them solely to release the app. Apple's bundle version and
Windows' numeric resource/installer version use the numeric core; full SemVer
stays in the executable, tag, filenames and update metadata.

Add useful user-facing bullets to `CHANGELOG.md` under **Unreleased / What's New**
as changes land. No raw commit list. Prepare each RC on a feature branch based
on `main`, following the normal PR flow:

```sh
# First candidate for the intended 0.1.0 stable release:
python3 scripts/release.py prepare 0.1.0-rc.1
python3 scripts/release.py validate v0.1.0-rc.1
# Review Cargo.toml, Cargo.lock and CHANGELOG.md, then commit via a PR.
```

The helper moves Unreleased into a dated section and leaves a fresh Unreleased
heading. It neither commits nor creates/pushes a tag. Empty/unfinished notes,
version mismatches and unprovisioned updater trust keys fail validation. After
fixes and useful Unreleased notes, prepare the next candidate:

```sh
python3 scripts/release.py prepare 0.1.0-rc.2
python3 scripts/release.py validate v0.1.0-rc.2
# Review and merge through a green PR as usual.
```

Every RC or beta gets its own useful changelog section. The first public stable
notes must also include the features from earlier unpublished drafts. The workflow
copies the version's section verbatim to GitHub Release notes and the signed
updater manifest. Edit it before tagging. Published versions and assets are immutable.

## Create the candidate tag and review the draft

After the preparation PR has merged and the **CI** workflow has passed for that
exact resulting `main` SHA, use a clean retained checkout:

```sh
git fetch origin main --tags
git switch main
git pull --ff-only origin main
python3 scripts/release.py validate v0.1.0-rc.1
gh run list --repo zevem/neptune --workflow ci.yml --commit "$(git rev-parse HEAD)"
# Confirm the exact main push run succeeded, then, only with release authorization:
git tag -a v0.1.0-rc.1 -m "Neptune 0.1.0-rc.1"
git push origin refs/tags/v0.1.0-rc.1
```

Subsequent candidates use the same commands with their `-rc.N` version.
Never move or force-push a release tag. A failed preflight can be rerun after
exact-commit CI completes; source corrections need the next RC tag.
Do not tag an unrelated feature branch.

`Desktop release` runs only on `push.tags: v*`:

1. Reject malformed SemVer, package/lock mismatches, missing dated notes or trust key.
2. Require that the commit is on main and a successful main-push CI run exists
   for that exact SHA. Reuse that evidence instead of repeating the whole CI suite.
3. Build optimized native binaries on Ubuntu 22.04 x64, macOS 15 ARM64,
   macOS 15 Intel, and Windows 2025 x64. A parallel Intel job tests the app
   library on that extra target. Builds start clean and use every runner core.
4. Package all installers with fonts/dependency/license notices. macOS signing,
   notarization and Gatekeeper assessment are mandatory. Windows is unsigned.
5. Only after **all** platform jobs succeed, aggregate and require exactly five
   artifacts, sign update metadata, generate/check SHA256SUMS, and attest final bytes.
6. Upload one private draft and verify the complete asset set. Its title/notes
   say **INCOMPLETE** until all uploads are verified. Upload failures leave that
   private marker; there is no automatic publish step. Reruns cannot mutate public releases.

| Platform | Artifact |
| --- | --- |
| macOS Apple Silicon | `Neptune-{version}-macos-arm64.dmg` |
| macOS Intel | `Neptune-{version}-macos-x64.dmg` |
| Windows x64 | `Neptune-{version}-windows-x64.exe` |
| Linux x64 portable | `Neptune-{version}-linux-x64.AppImage` |
| Linux x64 Debian/Ubuntu | `Neptune-{version}-linux-x64.deb` |

Also attached: `SHA256SUMS`, `update-manifest.json`, `update-manifest.sig`.
Linux targets glibc 2.35+; window libraries are bundled in AppImage, while glibc,
graphics loaders/drivers remain host-owned. DEB declares native dependencies.
DEB maps SemVer prereleases to `~beta.N` / `~rc.N` so they sort below stable.
AppImage tooling and its MIT runtime are version- and SHA-pinned; updating those
pins requires reviewing upstream primary sources and rerunning packaging proof.
Windows uses Inno Setup, per-user installation, Start Menu/optional desktop
shortcuts and uninstall registration. It preserves user settings/workspaces.

Browser downloads do not preserve Linux execute permission. Before testing a
downloaded AppImage, enable execution in file Properties or run `chmod u+x` on
that exact file, then launch it. Opening it with Gear Lever presents a management
screen; trusting and launching/integrating the file is a separate action.
Record downloaded-file permissions and test both the direct launch and any
integration tool used. A `--version` check alone does not verify a native window.
See the [download instructions](installation.md#linux-appimage).

Once the workflow is green, download draft artifacts as an authenticated operator,
verify all eight asset names, signatures/checksums/attestations, test installers
and run the [native acceptance gates](verification.md#remaining-production-release-gates).
Review the notes and confirm the RC has GitHub's prerelease checkbox enabled.
Record acceptance before proceeding to stable promotion. Keep the RC private
unless explicitly authorized to publish that preview; a published RC remains a
prerelease and is never marked latest. Prefer the GitHub UI, or, with explicit
release-publication authorization:

```sh
# Optional public RC; this does not publish a stable release:
gh release edit v0.1.0-rc.1 --repo zevem/neptune --draft=false --prerelease --latest=false
```

## Promote an accepted release candidate

1. Verify the latest RC for the intended `X.Y.Z` passed all applicable acceptance
   gates, and link its evidence from the stable preparation PR. Outstanding blockers
   or missing platform evidence keep the release in the RC cycle.
2. Prepare `X.Y.Z` on a feature branch based on `main`. Compared with the accepted
   RC, promotion changes only release version/notes metadata: no new application,
   dependency, packaging or workflow changes. Any such change needs another RC.
   Summarize all user-facing changes since the previous public stable release
   under Unreleased before running the helper; it does not collect RC notes for you.

   ```sh
   python3 scripts/release.py prepare 0.1.0
   python3 scripts/release.py validate v0.1.0
   ```

3. Review and merge through green PR CI, then require successful main-push CI for
   the exact resulting SHA. With authorization to release that stable version,
   use the tag procedure above with `v0.1.0` and message `Neptune 0.1.0`.
4. The workflow builds and signs new stable installers and leaves a private draft.
   Verify all eight assets and native acceptance of those exact final installers,
   including install/launch and version/update-channel behavior. RC evidence does
   not prove the final artifact bytes. A failed final draft stays private; preserve
   its occupied stable tag and start RCs for the next unused stable version.
5. With explicit stable-publication authorization, publish the complete accepted
   draft as stable and **latest**:

   ```sh
   gh release edit v0.1.0 --repo zevem/neptune --draft=false --prerelease=false --latest
   ```

Verify public GitHub downloads, `neptune.rs/download`, all five installer links,
and native update checks after publication; allow up to five minutes for website
caching. The Beta channel and `/download/beta` also carry published RCs according
to the channel rules below; private drafts are excluded from both channels.

## GitHub environment and signing credentials

Use **Settings → Environments → desktop-release**. Allow only tags matching `v*`.
Add required human reviewers where the team permits them, protect `v*` tags with
a repository ruleset, and restrict who can create tags/change workflow code and
secrets. Environment approval releases secrets to the workflow; it does not
publish the resulting draft. PR CI uses read-only permissions. Release jobs get
only their declared contents/actions/attestation/OIDC permissions; only final
staging has contents write. Third-party actions are pinned to full commit SHAs.
No signing credential belongs in the repository, artifact payload or logs.

Environment secrets:

| Secret | Value / scope |
| --- | --- |
| `UPDATE_SIGNING_KEY` | Ed25519 PKCS#8 private PEM; signs exact update-manifest bytes |
| `MACOS_CERTIFICATE_P12_BASE64` | Base64 exported **Developer ID Application** certificate and private key |
| `MACOS_CERTIFICATE_PASSWORD` | Password protecting that .p12 |
| `MACOS_SIGN_IDENTITY` | Exact Developer ID Application signing identity (or certificate SHA-1) |
| `APPLE_API_KEY_P8_BASE64` | Base64 private App Store Connect team API key (.p8) |
| `APPLE_API_KEY_ID` | Key's ID |
| `APPLE_API_ISSUER_ID` | Team API issuer UUID |

`GITHUB_TOKEN` is provided automatically by Actions. No personal release PAT is
needed. Website `GITHUB_RELEASES_TOKEN` is optional and belongs only to the
Vercel server environment: a fine-grained token limited to public repository
metadata/contents read, never Actions write or release signing credentials.
Public unauthenticated discovery works with the server cache; a read token can
improve rate limits. Never use a `NEXT_PUBLIC_` variable for that token.

The updater key was replaced before the first release on 2026-10-02. Its private
PEM was uploaded directly to GitHub `desktop-release`; the public raw key is in
`packaging/update-public-key.hex`. The website embeds that public key at build
time from the same file. The owner-requested backup is stored outside the
worktree in a permissions-restricted folder under Downloads on the setup host.
No private key was committed or printed. This local backup is not an offline
copy; retain an additional secure copy before relying on that host for recovery.
To generate another replacement **before first release**:

```sh
umask 077
openssl genpkey -algorithm ED25519 -out /secure/location/neptune-update.pem
openssl pkey -in /secure/location/neptune-update.pem -pubout -outform DER -out /secure/location/neptune-update-public.der
# Write only the raw 32-byte public key to the repository (review it in a PR).
python3 -c 'from pathlib import Path; p=Path("/secure/location/neptune-update-public.der").read_bytes(); assert p[:12].hex()=="302a300506032b6570032100"; Path("packaging/update-public-key.hex").write_text(p[12:].hex()+"\n")'
gh secret set UPDATE_SIGNING_KEY --repo zevem/neptune --env desktop-release < /secure/location/neptune-update.pem
```

Never replace the trust key arbitrarily after shipping: older apps authenticate
only their embedded key. A rotation needs a bridge release trusted by the old key
that adds the new trust anchor before switching signatures. If the old key is
lost/compromised, communicate an independently verified manual reinstall; do not
weaken signature checking. GitHub TLS and Ed25519 prove authenticity, but cannot
prevent a server/network from withholding a newer release.

## One-time Apple setup (owner action)

Apple credentials were provisioned on 2026-10-02 using the owner’s signed-in
Developer Program account. A new Developer ID Application certificate and its
matching private key were exported as a password-protected .p12; a dedicated
Developer-role team API key was created for notarization. All six Apple secrets
are stored in `desktop-release`, with owner-requested backups in a restricted
Downloads folder outside the worktree. Setup alone does not establish successful
notarization or Gatekeeper acceptance. For renewal or replacement, use a trusted
Mac and your existing Developer Program:

1. In Certificates, Identifiers & Profiles, create a **Developer ID Application**
   certificate (not Developer ID Installer or an App Store distribution certificate).
   Generate its CSR in Keychain Access, upload the CSR, download/install the issued
   certificate, and export the certificate **with its private key** as a
   password-protected .p12. Store a secure backup. If renewing an existing identity,
   verify the certificate is valid and includes the private key first.
2. In App Store Connect → Users and Access → Integrations → App Store Connect API,
   create a **team** API key for notarization. Apple's notarytool team-key flow
   needs the issuer ID and an appropriate role (use Developer where permitted).
   Avoid broader Admin access when Developer suffices. Download the .p8 once;
   record its key ID and issuer ID, and secure the backup. Do not paste them into chat.
3. Base64-encode the .p12 and .p8 without line wraps and upload through GitHub's
   environment secret UI, or pipe files to `gh secret set` locally. For example:

   ```sh
   base64 < /secure/location/DeveloperID.p12 | tr -d '\n' | gh secret set MACOS_CERTIFICATE_P12_BASE64 --repo zevem/neptune --env desktop-release
   base64 < /secure/location/AuthKey.p8 | tr -d '\n' | gh secret set APPLE_API_KEY_P8_BASE64 --repo zevem/neptune --env desktop-release
   # Enter password/identity/key ID/issuer ID through the secret UI or gh's prompt.
   gh secret set MACOS_CERTIFICATE_PASSWORD --repo zevem/neptune --env desktop-release
   gh secret set MACOS_SIGN_IDENTITY --repo zevem/neptune --env desktop-release
   gh secret set APPLE_API_KEY_ID --repo zevem/neptune --env desktop-release
   gh secret set APPLE_API_ISSUER_ID --repo zevem/neptune --env desktop-release
   ```

The runner imports the certificate into a temporary unlocked keychain, limits
private-key access to codesign, enables hardened runtime and secure timestamps,
signs the executable then app, submits the app ZIP with modern `notarytool`,
staples/validates the app, creates the drag-to-Applications DMG, signs/notarizes
and staples the DMG, and assesses app and DMG with `spctl`. No unnecessary sandbox,
JIT or library-validation entitlements are added. Temporary keys/keychain/files
are cleaned even on failure. Gatekeeper must also be tested on a real quarantined
browser download on each Mac architecture before publication.

Sources: [Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution),
[custom notarization](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow),
[Developer ID distribution](https://developer.apple.com/developer-id/),
[App Store Connect API keys](https://developer.apple.com/documentation/AppStoreConnectAPI/creating-api-keys-for-app-store-connect-api).

## Windows signing limitation

Windows Authenticode signing is deliberately **not implemented**. SmartScreen
may warn users, and the installer publisher is unverified. This is shown on the
website/native update sheet. Ed25519 update verification and GitHub provenance
still apply; they do not claim to replace Windows publisher reputation.
When SignPath OSS is configured, sign the executable/installer **before** final
checksums, manifest signing and attestations. Inno Setup's SignTool can sign the
installer/uninstaller. Add the signing job's completion to the release gate; keep
unsigned output out of the final artifact set when that policy becomes mandatory.

## Desktop update behavior

The updater is a desktop-owned service in `src/runtime/updates.rs`, with in-place
installation, relaunch and the manual native handoff in `src/platform/updates.rs`
and egui presentation in `src/ui/updates.rs`.
It adds no terminal/model/PTY dependency and has no custom update backend.

Preferences → Updates has **Check automatically** (default on), **Release channel**
(Stable by default) and manual check/review controls. A check runs after a
15-second launch grace period once restoration is ready, then at most once every 24 hours in
that session. Quiet frames sleep until that deadline or a worker completion.
Checks, downloads and hashing never run on an interactive frame. One worker slot,
bounded metadata/history/installer size and network deadlines prevent unbounded
queues. Cancel/channel changes invalidate results and keep that slot reserved
until the old worker exits; closing the app cancels it without waiting on network.
Automatic failures stay in Preferences, without repeatedly interrupting the shell.

Stable queries GitHub's latest published stable release. Beta considers published
stable and prerelease releases by **SemVer precedence**; a final `0.2.0` supersedes
`0.2.0-beta.N`, and a newer `0.3.0-beta.N` can follow it. Neither channel downgrades,
offers a same-version build-metadata change, or accepts a misclassified/draft tag.
Beta installation does not silently change the default preference; choose Beta
explicitly to continue previews. Switching back to Stable waits for a stable newer
than the installed beta, rather than reinstalling an older stable.

The manifest binds repository, schema, full version/tag, source commit, human
release notes and all five artifact names/sizes/hashes. Neptune verifies its
Ed25519 signature with the embedded trust anchor **before showing an offer**,
then checks exact SHA-256 and size after downloading, and checks again before
handoff. Artifact URLs are derived only from that authenticated tag/name in the
official repository, use HTTPS, bounded redirects and TLS validation. A DMG the
user opens is marked quarantined as a download; one Neptune installs itself is
not, as its bytes are already authenticated. GitHub attestations provide
additional independently verifiable workflow provenance.

A quiet notification offers **What's New / Later** without taking terminal focus.
The native sheet shows version, authenticated notes, release link and install
instructions. Download and install are separate intentional actions. Closing the
sheet, Escape or Later suppresses that version's notification for the session;
manual review remains available.

A macOS app bundle and a Linux AppImage update in place: after the download is
verified, **Restart to update** replaces the installation and restarts Neptune.
No new artifact or manifest field is involved; the same signed DMG/AppImage is
used.

- **Linux AppImage:** the verified file is copied beside the running AppImage
  (the path the AppImage runtime reported at launch), given its permissions and
  renamed over it. The path, and any desktop integration that points at it, is
  unchanged; a version in the file's name is not updated.
- **macOS:** Neptune mounts the verified DMG without showing it, copies
  `Neptune.app` into a hidden folder beside the installed bundle, requires
  `codesign --verify --deep --strict` to pass, then swaps the two bundles and
  removes the old one. A failed swap restores the installed bundle.

The replacement happens while Neptune is still running, so a failure is reported
in the sheet with the installation untouched. Neptune then quits through the
usual close confirmation, including the running-process warning, and starts the
new version with the same `--config`/`--data-root` once state is flushed and
sessions are shut down. Workspaces return with fresh shells, as on any launch.
Cancelling that confirmation keeps the old process running with the new version
on disk; Preferences → Updates and the sheet offer **Restart to update**, and
any later launch starts the new version. An installation cannot be cancelled
midway, and the old process stops checking for updates once it has installed one.

Where Neptune cannot write (a read-only volume, a translocated or unowned
`/Applications` bundle, a root-owned AppImage), the sheet says so and falls back
to the manual handoff: macOS opens the verified DMG, and Linux shows the verified
file in a file manager. Windows always launches the verified interactive
installer and DEB installations show the DEB; neither is replaced in place.
Source builds outside an app bundle or AppImage choose the manual AppImage/DMG
handoff. Neptune never replaces itself or closes PTYs without the user asking,
and never requests elevated installation.
Downloads live in a private temporary directory and are removed on failure,
cancel, exit or successful in-place installation; after a manual handoff they
are retained for the installer/file manager and normal OS/user cleanup.

## Website downloads

Vercel already hosts neptune.rs. The site now uses Next.js server cache/routes,
not `output: export`; retain the Vercel **website** root directory. Committed
`website/vercel.json` selects Next.js, frozen Bun install, `bun run build` and
`.next` output, overriding an old static-output setting. Verify **Include source
files outside of the Root Directory in the Build Step** is enabled in Vercel's
Root Directory settings: the build reads the shared public trust key from
`packaging/`. Check this flag on the first preview deployment.
No new service or binary mirror is needed.
The landing page links to `/download` and keeps the source-build instructions.

- `/download`: prefers the published stable release from GitHub `/releases/latest`.
  Before the first stable release is published, it shows the newest Beta release,
  labeled Beta and using Beta installer routes. The Stable/Beta choice appears
  once a verified stable release is available. A failed Stable lookup retains
  the retry state rather than establishing that Stable is empty.
- `/download/beta`: newest **published prerelease by publication time**; no stable
  fallback that could mislabel a download as Beta.
- `/download/file/{platform}` and `/download/beta/file/{platform}`: temporary
  redirects to the canonical asset. UI links bind `?tag=v...` to the displayed
  version so release rollover cannot mismatch displayed checksums and downloads.
  Without `tag`, these Neptune-owned links track the latest channel release.
  Stable installer routes never fall back to prereleases.

Server-side Next `use cache` plus fetch caching coalesces/caches metadata for five
minutes; empty/error states are cached for one minute. Requests are bounded and
credentials never reach clients or asset hosts. The loader verifies the same
Ed25519 manifest and complete artifact set. An incomplete/forged release shows
unavailable, not partially working download buttons. No published release shows
an honest empty state. Temporary API failures offer retry/GitHub Releases.

Browser OS/architecture hints recommend the appropriate x64 or ARM64 artifact.
Safari/other browsers can hide Mac CPU architecture; ambiguous Macs get explicit
Apple Silicon/Intel choices and About This Mac guidance rather than a guessed
binary. ARM Linux/Windows and mobile browsers get manual choices without an
unsupported native recommendation. All five packages are always visible. The
page shows version/channel/format, notes, source commit, release, SHA-256 and
verification links. No version is hardcoded in the download UI.

Hosting references: [Vercel configuration](https://vercel.com/docs/project-configuration/vercel-json),
[monorepo source files](https://vercel.com/docs/monorepos/monorepo-faq).

## Verify a download

Download `SHA256SUMS` and the desired installer from the same published release:

```sh
# Linux:
sha256sum --ignore-missing -c SHA256SUMS
# macOS (checks only the file you've downloaded):
shasum -a 256 Neptune-0.1.0-macos-arm64.dmg
# Windows PowerShell:
Get-FileHash .\Neptune-0.1.0-windows-x64.exe -Algorithm SHA256
# Compare with its entry in SHA256SUMS, then verify the workflow provenance:
gh attestation verify ./Neptune-0.1.0-macos-arm64.dmg --repo zevem/neptune --signer-workflow zevem/neptune/.github/workflows/release.yml
```

Use the actual downloaded version/platform filename. A checksum detects corruption;
use attestation verification for repository/workflow provenance, or authenticate
the Ed25519 manifest against the public key obtained from a trusted Neptune build.
[GitHub artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
cover the final artifacts, metadata and checksum file. The draft staging job is
the attested workflow: it consumes the native jobs' uploaded artifacts from the
same workflow run and signs only the complete set.

## Troubleshoot a failed release

- **Acceptance:** keep a rejected draft private and follow the
  [RC policy](#release-candidate-policy); fixes increment the RC number for the
  intended stable version. If a rejected stable tag already exists, preserve it
  and begin RCs for the next unused stable version.
- **Preflight:** check strict tag/package/lock equality, dated What's New bullets,
  main ancestry, trust key and exact main SHA's successful CI run. Wait for CI and
  rerun if it was pending. Do not disable checks or force-move the tag.
- **Apple:** check certificate expiry/private-key export, identity, team-key role,
  key ID and issuer. OpenSSL 3's default PKCS#12 protection may fail Keychain
  import even with the correct password. Prefer a Keychain Access export; for an
  OpenSSL export use `-keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1`
  with the matching Developer ID intermediate certificate included. Keep the
  encrypted original private key backup. See [Apple's compatibility explanation](https://developer.apple.com/forums/thread/723242)
  and [OpenSSL export parameters](https://docs.openssl.org/3.5/man1/openssl-pkcs12/).
  Retrieve `xcrun notarytool log <submission-id> --key <secure-p8>
  --key-id <id> --issuer <issuer>` on a trusted Mac. First submissions can take
  longer than 30 minutes; inspect Apple's status/history and rerun as appropriate.
  Never bypass notarization/stapling or publish unsigned DMGs.
- **Linux:** inspect unresolved `ldd` dependencies, bundled copyright files,
  pinned tooling checksums and AppImage `--version` smoke output. A pinned upstream
  download disappearing should fail closed; review/update its version and digest
  in a PR. Test Wayland/X11 and graphics drivers on the target distributions.
- **Windows:** confirm the runner has Inno Setup 6, valid numeric version and
  payload paths. Add signing only after SignPath OSS configuration is approved.
- **Manifest:** verify the environment PEM corresponds to the committed raw public
  key; check exactly five artifact names and nonzero bounded sizes. No unsigned fallback.
- **Upload:** failed uploads remain private drafts. Wait for/fix the failed job,
  rerun only before publication, then verify eight assets and the green workflow.
  GitHub's by-tag REST endpoint returns published releases only. Operators can
  inspect a draft with `gh release view`; staging lists authenticated releases
  with pagination so existing drafts are found and verified without publication.
  See [GitHub release lookup semantics](https://docs.github.com/en/rest/releases/releases#get-a-release-by-tag-name).
  Do not publish a draft from a failed/cancelled run. Public release reruns are refused.
- **Website:** inspect Vercel runtime logs/configuration, API limits, public-key
  build value and release classification/completeness. Wait up to five minutes
  after publication; empty/error state retries after one minute. Never expose a
  private GitHub token while debugging.
- **Desktop:** manual check surfaces connection/integrity failures; cancelled
  checks drain their worker before retry. Compare the channel and running SemVer.
  Do not bypass a signature/hash rejection. A bad release should be withdrawn
  from recommendation and superseded with a higher, newly tagged version.

Local proof is focused, not a workspace-wide Rust sweep:

```sh
python3 -m unittest discover -s scripts -p test_release.py
python3 scripts/check-architecture.py
cargo test -p neptune-terminal --lib runtime::updates::tests --locked
cargo test -p neptune-terminal --lib app::tests::update_channel_preferences --locked
# In website/:
bun install --frozen-lockfile
bun run test
bun run lint
bun run build
```

Workflow syntax should also pass actionlint. Native visual/installer/signing checks
need their actual hosts and isolated storage; see [the script guide](../scripts/AGENTS.md).
No real release/tag/publication is needed to exercise these regression tests.
