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
blank. Addresses, history and page contents are never serialized in workspace
state. The address and profile of each tab in a saved profile are kept beside
the profiles, in `<data>/browser/tabs.json` (at most 64 tabs, written off the
UI thread a second after a change), so a restored tab opens where it was; a
private tab is never written there and closing a tab forgets it. Tabs were
restored blank until this was asked for. The configuration names the browser profiles; their cookies and site
data are Chromium's own files under the data directory.

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
Neptune bridge, shell access or model commands. Certificate checks, web security and
the Chromium sandbox retain their defaults, except on a Linux system that allows
Chromium no sandbox (see below). Popup links with a user gesture
stay in the preview; unsolicited windows are rejected. DevTools opens a CEF
window. Platform sandbox setup follows [CEF's official requirements](https://chromiumembedded.github.io/cef/sandbox_setup).

## Profiles, cookies, picking and recording

Browser profiles were in-memory until they were made to keep sign-ins on
request; **Private** and `browser_profile = "private"` keep the earlier
behaviour. A saved profile is a CEF request context whose cache path is a
direct child, `profile.<id>`, of `<data>/browser`, the helper's root cache
path; a path further down is silently in-memory. A disk context initializes
asynchronously, so the helper holds a tab's creation until it has. One Neptune
process holds `<data>/browser/lock`; others, screenshot captures and a failed
lock use a temporary root that the supervisor removes even after helper
crashes, and only private tabs. Removing a profile writes
`<data>/browser/removed/<id>`; the directory is deleted before the next helper
starts, when Chromium no longer has it open. A tab's profile is fixed when its
browser is created: changing it restarts the pane. Cookies and site data are
unencrypted Chromium files readable by the user account, like a browser's own.

Cookie import reads another browser's cookie database on a desktop worker
from a private snapshot, never the live file, refuses a browser that is
running, and sends cookies to the helper in batches of 64 to set through CEF's
cookie manager. Chromium-family values are decrypted with the key from the
Secret Service on Linux or the Keychain on macOS; Windows app-bound encryption
and Safari are unsupported. Cookie names and values are never logged or shown;
the result is a count. The readers have run on Linux against generated
databases and a stand-in Secret Service; a real keyring, macOS and Windows
remain to be accepted.

Picking evaluates a script in the page's own JavaScript world through DevTools
and awaits its result. The page can therefore see and alter it: the result is
untrusted text, capped at 16 KiB, with every part bounded and control
characters removed before it reaches the clipboard, and never interpreted. The
picture is cropped by the supervisor from its own last frame. The script
installs no bridge and is removed when the pick ends.

Recording asks DevTools for a JPEG screencast of the page and feeds the frames
to a hidden helper page that encodes VP9 WebM with MediaRecorder. The helper
writes the file, to a path the supervisor chose, creating it new. Recordings
stop after ten minutes, when their tab closes and when the helper exits. They
carry no audio and no duration header.

Previews paint on white where a page sets no background, as browsers do;
earlier the pane's own background showed through.

The CEF build in use paints an offscreen page at one pixel per view unit
whatever `device_scale_factor` its screen information reports (measured on
Linux with CEF 154: `devicePixelRatio` stayed 1 and frames kept the view's
size at 1.5 and 2). The helper therefore sizes the view in the pane's physical
pixels and zooms the page by the display's scale, reapplied on each main-frame
navigation because Chromium keeps zoom by site. The page gets the layout width
and `devicePixelRatio` of a window on that display, and a frame meets the
screen one to one. Pointer positions and wheel distances cross the pipe in
logical points and are scaled in the helper. A touchpad's distances are sent
as precise deltas, with what falls short of a pixel carried to the next
event; a wheel's notches are 53 points each and left to Chromium to ease.

The pointer a page asks for, the tooltip of the element under it and the
address of a hovered link travel in the pane's state as a cursor type and two
bounded single-line strings; they are page data and are only ever shown. A
saved profile opens asynchronously, so the helper holds a tab's size,
visibility, focus, frame rate and first address until its browser exists.

A page's icon is downloaded by Chromium, shrunk to at most 64 pixels a side and sent
as a bitmap for the tab; it is page data and is only ever drawn.

## Development and verification

Build sequentially with the pinned toolchain and configured job limit:

```sh
cargo build -p neptune-browser --locked
cargo build -p neptune-terminal --features inspection --locked --bin neptune --bin neptune-inspect
```

CEF's initial download is large. CMake and Ninja are required. A validated
distribution may be reused with `CEF_PATH`; its archive must match the lockfile.
Linux builds also need the GLib development library (`libglib2.0-dev` on Debian/Ubuntu)
for native event dispatch. Linux needs the Chromium runtime libraries (NSS/NSPR, ATK, CUPS, GBM, Pango,
Cairo, ALSA and X11). A system with restricted user namespaces also needs a
root-owned mode-4755 `chrome-sandbox`; the DEB packages that helper. AppImage
uses the host's user namespace support, and an AppImage mount cannot provide a
setuid helper. Chromium aborts at startup with neither, which is every AppImage
launch on Ubuntu 24.04 and later, where AppArmor denies the namespace its
capabilities.

Browser previews must work without setup, so before each host launch the
supervisor checks, on its worker, for a valid helper beside the host and
whether a child may map its user in a new user namespace. Only when the system
allows neither does it start the host with `--no-sandbox`; a page then runs
with the user's access, and a blank preview says that pages run without
Chromium's sandbox. A system that allows either keeps the sandbox, and
[the installation guide](installation.md#browser-previews-and-chromiums-sandbox)
says how to allow one. Never disable the sandbox to work around any other
failure. The browser error screen keeps terminals usable.

Neptune restores the launch environment before it starts children, so an
AppImage's host loads the system's Chromium runtime libraries, which match its
graphics drivers and NSS database. When glibc's loader reports that the system
lacks one of them, the supervisor starts the host with the AppImage's `usr/lib`
first in its library path. That bundle carries the NSS modules NSS opens at
runtime, since a newer system's modules do not load into it.

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
