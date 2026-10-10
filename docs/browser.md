# Embedded project previews

The browser is a pane kind in Neptune's native tab/split layout, as requested
for previews beside terminal tabs. The terminal and the application chrome
remain native Rust/egui/wgpu. Chromium is loaded only when a browser pane opens,
in a separate `neptune-browser` process. The desktop executable has no CEF
dependency. CEF is pinned to 154.0.34 / Chromium 154.0.8037.98 through the locked
`cef` 154.5.0 crate. Binary runtime assets and CEF/Chromium license notices are
included in packaging; this substantially increases installer size.
Linux packaging removes unmapped debug and static symbol sections from its CEF
copy; the downloaded SDK remains intact.

The pure model owns `PaneKind`, generation, lifecycle and layout. Its existing
move/split/close commands remain the layout authority. Workspace schema 14
stores pane kind, defaults earlier panes to terminals and restores browsers
blank. Addresses, history, profiles and page contents are never serialized.

The desktop supervises one helper per window, with a maximum of eight browser
panes. Pipe commands are bounded to 64; messages are capped at 512 KiB and
text operations at 64 KiB. On Linux the worker attempts a bounded 1 MiB pixel
pipe capacity to reduce large-frame context switches; restricted kernels retain
the original OS capacity. The helper uses a binary buffered pipe writer without
text newline scanning. State and damaged pixel regions coalesce by pane generation. Both sides preserve the
union of pending damage, so a slow consumer never loses intervening changes.
A bounded, complete worker-owned surface supplies unchanged pixels between
regions; texture updates upload only that union. Creation, resizing and showing
a hidden preview send a complete surface. Closed and stale generations
cannot replace a new preview. Startup, page scripts, pipe I/O and BGRA conversion
run outside interactive frames. The UI consumes owned textures and metadata.
Hidden tabs and minimized or covered windows keep their web sessions but stop
painting. Restoring a preview requests a complete surface before partial updates. The isolated CEF event
loop follows CEF deadlines, with a 33 ms idle fallback based on [CEF's external-pump example](https://github.com/chromiumembedded/cef/blob/master/tests/shared/browser/main_message_loop_external_pump.cc);
while a visible preview is painting, the fallback shortens to a quarter of its
frame interval (bounded to 1–8 ms) to service asynchronous GPU/native work.
After 100 ms without paint, or immediately when hidden, it returns to the idle
fallback. This does not request a desktop frame or upload an unchanged texture.
Linux also dispatches a bounded, nonblocking batch of native GLib events, as
in [CEF's Linux pump](https://github.com/chromiumembedded/cef/blob/master/tests/shared/browser/main_message_loop_external_pump_linux.cc),
so clipboard selection requests and DevTools events remain responsive.
The helper exits after the last browser closes, with bounded teardown and Unix process-group cleanup.

CEF offscreen rendering follows the current window monitor's refresh rate,
rounded to whole frames per second, with a 240 fps resource cap and a 60 fps
fallback if the platform cannot report the rate. Moving between monitors updates
the rate for existing previews. These are generation limits, not guaranteed
presentation rates; rendering complexity, DPI and host load still matter.
Surfaces are capped at four megapixels and 4096 pixels per side. High-DPI
previews reduce their render scale to stay within that bound. Pixel conversion
uses one pass on the worker and reuses its pipe buffer. Complete frame snapshots
share immutable pixels with the worker cache; later damage preserves any frame
still being uploaded before mutating the cache. Existing main/popup GPU
textures receive partial updates rather than replacement on every frame.

This remains a bounded CPU copy/upload path, rather than shared GPU textures.
CEF shared-texture callbacks need copying or importing within their callback
lifetime, and require separate platform/resource validation before adoption.
The native probe below measures changing pixels in the actual application
window separately from the page's requestAnimationFrame rate; neither metric
measures photon latency.

Only HTTP, HTTPS and a blank page are navigable. Page JavaScript receives no
Neptune bridge, shell access or model commands. Profiles are in-memory with a
unique temporary root for CEF bookkeeping, owned and removed by the supervisor
even after helper crashes. Certificate checks, web security and
the Chromium sandbox retain their defaults. Popup links with a user gesture
stay in the preview; unsolicited windows are rejected. DevTools opens a CEF
window. Platform sandbox setup follows [CEF's official requirements](https://chromiumembedded.github.io/cef/sandbox_setup).

## Development and verification

Build sequentially with the pinned toolchain and configured job limit:

```sh
cargo build -p neptune-browser --locked
cargo build -p neptune-terminal --features inspection --locked --bin neptune --bin neptune-inspect
```

CEF's initial download is large. CMake and Ninja are required. A validated
distribution may be reused with `CEF_PATH`; its archive must match the lockfile.
Linux needs the Chromium runtime libraries (NSS/NSPR, ATK, CUPS, GBM, Pango,
Cairo, ALSA and X11). A system with restricted user namespaces also needs a
root-owned mode-4755 `chrome-sandbox`; the DEB packages that helper. AppImage
uses the host's user namespace support. Never work around a sandbox failure by
disabling it. The browser error screen keeps terminals usable.

macOS packaging places the CEF framework and role-specific helper apps in
`Contents/Frameworks`, initializes the helper sandbox before loading CEF and
signs nested browser executables with Chromium JIT entitlements. Windows uses
CEF's sandbox `bootstrapc.exe`, renamed `neptune_browser.exe`, with the Rust
`neptune_browser.dll`. For a Windows development launch, prepare the private
`browser/` payload using `package-common.browser_payload` beside Neptune.

Linux native rendering and input, packaged launch, idle/busy resource behavior,
close/restart, hidden tabs, narrow panes and native visual acceptance must be
recorded for the final build. macOS and Windows compilation, sandbox behavior,
clipboard/IME, DPI and installer launch require native acceptance on those hosts.
Configured CI and packaging paths do not establish that acceptance.

### Browser frame pacing

On Linux/X11 or XWayland, use an optimized inspection build and the packaged,
sandbox-enabled browser payload. The probe launches its own app with fresh
storage, a unique inspection endpoint and a local animated page, selects only
its PID-owned window, and stops only its captured process. Run desktop input
probes sequentially.

```sh
python3 scripts/measure-browser.py --host /path/to/browser/neptune-browser \
  --output artifacts/browser-motion --scene motion --min-fps 125
python3 scripts/measure-browser.py --host /path/to/browser/neptune-browser \
  --output artifacts/browser-full --scene full --min-fps 125
```

Choose the FPS threshold for the display (for example, 50 on a 60 Hz monitor).
The window moves onto the primary monitor by default; `--position X,Y` chooses
another location without altering monitor settings. `--size WIDTHxHEIGHT`
exercises narrow/large previews. `--max-frame-ms` can impose a 95th-percentile
interval limit. Results include page animation rate, native window pixel-change
rate, interval distributions, sampling frequency, owned process CPU, binary
hashes, display modes and a running-app screenshot. Static/hidden resource and
input/lifecycle checks remain separate from this motion probe.
