#!/usr/bin/env python3
"""Measure preview pacing in a PID-owned native X11 window, not just page RAF.

Requires an optimized inspection build, a sandbox-enabled browser payload,
psutil, X11/XWayland and xrandr. No user profiles, pages or terminal contents
are accessed. Pixel changes measure window contents, not photon latency.
"""
from __future__ import annotations

import argparse
import ctypes as C
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import threading
import time

import psutil

ROOT = Path(__file__).resolve().parents[1]
PAGE = """<!doctype html><meta charset=utf-8><title>Frame pacing probe</title>
<style>
body{margin:0;background:#10131b;color:#e8eaf2;font:16px system-ui}
#mark{position:fixed;left:0;top:0;width:64px;height:64px}
article{padding:96px 32px}h1{font-size:32px}p{color:#9daac0}
.track{height:100px;position:relative;background:#172338;border-radius:16px;margin-top:32px}
.dot{position:absolute;left:12px;top:26px;width:48px;height:48px;border-radius:50%;background:#54b6ff;will-change:transform}
small{color:#729cc1}
</style><canvas id=mark width=64 height=64></canvas><article>
<small>NEPTUNE / NATIVE FRAME PACING</small><h1>Smooth project previews</h1>
<p>A local animation measured through the native window.</p>
<div class=track><div class=dot></div></div><p id=rate>Starting measurement…</p></article>
<script>
const scene = 'SCENE';
let g=mark.getContext('2d'),n=0,begin=performance.now(),previous=begin,intervals=[],ready=false;
function paint(t) {
  n++;
  if(scene==='full')document.body.style.backgroundColor='rgb('+((n*17)%256)+',20,40)';
  g.fillStyle='rgb('+((n*37)%256)+',60,180)';g.fillRect(0,0,64,64);
  if(scene!=='marker')document.querySelector('.dot').style.transform=
    'translateX('+((Math.sin(t/750)+1)/2*Math.max(0,innerWidth-120))+'px)';
  intervals.push(t-previous);previous=t;
  if(!ready){ready=true;fetch('/ready',{method:'POST',body:'ready'})}
  requestAnimationFrame(paint);
}
requestAnimationFrame(paint);
setInterval(()=>{
  let now=performance.now();
  fetch('/stats',{method:'POST',body:JSON.stringify({frames:n,elapsed:now-begin,intervals:intervals.splice(0)})});
  rate.textContent=(n/((now-begin)/1000)).toFixed(1)+' browser animation frames / second';
},250);
</script>"""


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


def percentile(values, quantile):
    ordered = sorted(values)
    return ordered[int((len(ordered) - 1) * quantile)] if ordered else None


def pair(value, separator):
    values = tuple(map(int, value.split(separator)))
    if len(values) != 2:
        raise ValueError("Expected two coordinates/dimensions")
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=ROOT / "target/release/neptune")
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/neptune-inspect")
    parser.add_argument("--host", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scene", choices=["marker", "motion", "full"], default="motion")
    parser.add_argument("--seconds", type=float, default=8)
    parser.add_argument("--min-fps", type=float, default=125)
    parser.add_argument("--max-frame-ms", type=float, help="Maximum allowed native p95 interval")
    parser.add_argument("--size", default="1180x760")
    parser.add_argument("--position", help="X,Y in desktop pixels; defaults to primary monitor center")
    args = parser.parse_args()
    if not os.environ.get("DISPLAY"):
        parser.error("An X11/XWayland display is required")
    if not 2 <= args.seconds <= 12 or args.min_fps <= 0:
        parser.error("Use 2–12 sampling seconds and a positive FPS threshold")
    logical_width, logical_height = pair(args.size, "x")
    if logical_width < 640 or logical_height < 400:
        parser.error("Neptune's minimum window size is 640x400")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    data = out / "data"
    # Refuse reused data roots, even when a previous measurement failed.
    data.mkdir()
    (data / "config.toml").write_text('shell = "/bin/sh"\nshell_args = []\ncheck_updates = false\ndesktop_notifications = false\n')
    harness = module("browser_native_harness", ROOT / "scripts/native-harness.py")
    native = module("browser_native_input", ROOT / "scripts/native-smoke.py")
    lock, started, stats = threading.Lock(), threading.Event(), []

    class Server(http.server.BaseHTTPRequestHandler):
        def log_message(self, *unused):
            pass

        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/html;charset=utf-8")
            self.end_headers()
            self.wfile.write(PAGE.replace("SCENE", args.scene).encode())

        def do_POST(self):
            body = self.rfile.read(min(int(self.headers.get("Content-Length", 0)), 65536))
            if self.path == "/ready":
                started.set()
            elif self.path == "/stats":
                with lock:
                    stats.append(json.loads(body))
                    del stats[:-64]
            self.send_response(204)
            self.end_headers()

    result = {
        "status": "running", "scene": args.scene, "size": args.size,
        "host": os.uname().release, "display": os.environ["DISPLAY"],
        "seconds": args.seconds, "minimum_native_fps": args.min_fps,
        "app_sha256": hashlib.sha256(args.app.read_bytes()).hexdigest(),
        "helper_sha256": hashlib.sha256(args.host.read_bytes()).hexdigest(),
        "lockfile_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
        "max_frame_ms": args.max_frame_ms,
    }

    def save():
        (out / "result.json").write_text(json.dumps(result, indent=2))

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Server)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    app = log = x11 = None
    try:
        address = harness.free_endpoint()
        env = {**os.environ, "EGUI_INSPECTION": address,
               "NEPTUNE_BROWSER_HOST": str(args.host.resolve()), "PS1": "probe $ "}
        env.pop("WAYLAND_DISPLAY", None)
        log = (out / "app.log").open("w")
        app = subprocess.Popen([
            str(args.app.resolve()), "--data-root", str(data), "--cwd", str(data),
            "--no-restore", "--diagnostics", "--size", args.size,
            "--browser", f"http://127.0.0.1:{server.server_port}/",
        ], env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        result.update(pid=app.pid, address=address, data_root=str(data))
        save()
        harness.wait_ready(app, args.client, address, 30)
        if not started.wait(25):
            raise RuntimeError("Owned animation producer did not start")
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], env=env, text=True)
        window = next(int(w, 16) for w in re.findall(r"(0x[0-9a-f]+)", tree)
                      if f"= {app.pid}\n" in subprocess.run(
                          ["xprop", "-id", w, "_NET_WM_PID"], env=env, capture_output=True, text=True).stdout)
        x11 = native.X11()
        width, height = x11.geometry(window)
        modes = subprocess.check_output(["xrandr", "--current"], env=env, text=True)
        if args.position:
            position = pair(args.position, ",")
        else:
            primary = re.search(r"connected primary (\d+)x(\d+)\+(-?\d+)\+(-?\d+)", modes)
            if not primary:
                raise RuntimeError("No primary monitor reported; pass --position X,Y")
            mw, mh, mx, my = map(int, primary.groups())
            position = (mx + max(0, (mw - width) // 2), my + max(0, (mh - height) // 2))
        x11._declare("XMoveWindow", C.c_int, [native.Display, native.Window, C.c_int, C.c_int])
        x11.lib.XMoveWindow(x11.display, window, *position)
        x11.focus(window)
        time.sleep(1.5)
        tree = harness.inspect(args.client, address, "tree")["Tree"]
        nodes = [n for _, n in tree["accesskit"]["nodes"]]
        bounds = next(n["properties"]["bounds"] for n in nodes if n["properties"].get("label") == "Browser pane 2")
        width, height = x11.geometry(window)
        scale = tree["pixels_per_point"]
        x, y = int((bounds["x0"] + 16) * scale), int((bounds["y0"] + 16) * scale)
        # A real pointer click activates the window on GNOME/XWayland; an
        # EWMH focus request alone can be denied/reset by the compositor.
        x11.click(window, int((bounds["x0"] + 80) * scale), y, 1)
        time.sleep(.5)
        result.update(window_id=hex(window), window_pixels=[width, height], position=position,
                      sample_pixel=[x, y], content_bounds=bounds, pixels_per_point=scale, display_modes=modes)

        def pixel():
            pointer = x11.lib.XGetImage(x11.display, window, x, y, 1, 1, C.c_ulong(-1).value, 2)
            if not pointer:
                raise RuntimeError("Native pixel unavailable")
            try:
                value = x11.lib.XGetPixel(pointer, 0, 0)
                return (value >> 16) & 255, (value >> 8) & 255, value & 255
            finally:
                x11.lib.XDestroyImage(pointer)

        initial = pixel()
        if abs(initial[1] - 60) > 3 or abs(initial[2] - 180) > 3:
            raise RuntimeError(f"Sample is not the owned animation marker: {initial}")
        with lock:
            before = stats[-1].copy()
        family = [psutil.Process(app.pid), *psutil.Process(app.pid).children(recursive=True)]
        cpu_before = {pr.pid: sum(pr.cpu_times()[:2]) for pr in family}
        changes, polls, last = [], [], None
        begin = time.perf_counter()
        while time.perf_counter() - begin < args.seconds:
            color = pixel()
            now = time.perf_counter()
            polls.append(now)
            if color != last:
                changes.append(now)
                last = color
            time.sleep(.0015)
        finish = time.perf_counter()
        if len(changes) < 2:
            raise RuntimeError("Native marker stopped updating")
        time.sleep(.3)
        with lock:
            after = stats[-1].copy()
            raf = [dt for s in stats if before["elapsed"] < s["elapsed"] <= after["elapsed"] for dt in s["intervals"] if dt > 0]
        intervals = [(b - a) * 1000 for a, b in zip(changes, changes[1:])]
        poll_intervals = [(b - a) * 1000 for a, b in zip(polls, polls[1:])]
        cpu, rss = [], 0
        for pr in family:
            try:
                used = max(0, sum(pr.cpu_times()[:2]) - cpu_before[pr.pid])
                cpu.append({"pid": pr.pid, "name": pr.name(), "cpu_percent": used / (finish - begin) * 100})
                rss += pr.memory_info().rss
            except psutil.Error:
                pass
        result.update(
            native_fps=(len(changes) - 1) / (changes[-1] - changes[0]), native_frames=len(changes),
            native_interval_ms={"p50": percentile(intervals, .5), "p95": percentile(intervals, .95), "max": max(intervals)},
            browser_raf_fps=(after["frames"] - before["frames"]) / ((after["elapsed"] - before["elapsed"]) / 1000),
            browser_raf_interval_ms={"p50": percentile(raf, .5), "p95": percentile(raf, .95)},
            poll_hz=len(polls) / (finish - begin),
            poll_interval_ms={"p50": percentile(poll_intervals, .5), "p95": percentile(poll_intervals, .95), "max": max(poll_intervals)},
            native_long_gaps_ms=[gap for gap in intervals if gap > 30], cpu_by_process=cpu,
            family_cpu_percent=sum(pr["cpu_percent"] for pr in cpu), family_rss_sum_bytes=rss,
        )
        # RSS adds shared Chromium pages multiple times; it is not private memory.
        harness.inspect(args.client, address, "screenshot", str(out / "native.png"))
        passed = result["native_fps"] >= args.min_fps
        if args.max_frame_ms is not None:
            passed &= result["native_interval_ms"]["p95"] <= args.max_frame_ms
        result["status"] = "passed" if passed else "failed"
        print(json.dumps({k: result[k] for k in ("status", "native_fps", "browser_raf_fps", "native_interval_ms", "poll_hz", "poll_interval_ms", "family_cpu_percent")}, indent=2), flush=True)
    except BaseException as error:
        result.update(status="error", error=str(error))
        raise
    finally:
        if app:
            # SIGTERM does not run Rust's temporary-profile destructor. Capture
            # only this launch's descendants and supervised profile before it
            # exits, reap those handles, then remove its private fixture profile.
            owned, profiles = [], set()
            if app.poll() is None:
                try:
                    owned = psutil.Process(app.pid).children(recursive=True)
                    for process in owned:
                        try:
                            if Path(process.exe()) == args.host.resolve() and len(process.cmdline()) == 1:
                                profile = process.environ().get("NEPTUNE_BROWSER_PROFILE")
                                if profile:
                                    profiles.add(Path(profile))
                        except (psutil.Error, OSError):
                            pass
                except psutil.Error:
                    pass
            harness.stop_owned(app)
            _, remaining = psutil.wait_procs(owned, timeout=3)
            for process in remaining:
                try:
                    process.kill()
                except psutil.NoSuchProcess:
                    pass
            psutil.wait_procs(remaining, timeout=3)
            for profile in profiles:
                if profile.exists():
                    shutil.rmtree(profile)
        if log:
            log.close()
        if x11:
            x11.lib.XCloseDisplay(x11.display)
        server.shutdown()
        server.server_close()
        save()
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
