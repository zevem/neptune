#!/usr/bin/env python3
"""Verify workspace tear-off with real Linux/X11 input in an isolated Neptune.

Owns its process, storage, inspection endpoint and native window IDs. Inspection
is used only before the first tear-off: its protocol has no viewport selector.
Subsequent input and captures use X11 windows with this process's exact PID.
Requires an inspection build, a running X11/XWayland display, libXtst and Pillow.
The coordinate workflow measures the bootstrap window's display scale. An
optional caller-selected native desktop point and monitor region exercise a held
drag onto another monitor, verified from the actual native window position.
This proves Linux/X11 behavior; it does not establish Wayland/macOS/Windows or
mixed-DPI correctness. Review the native screenshots yourself.
"""

from __future__ import annotations

import argparse
import ctypes as C
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import shlex
import signal
import subprocess
import sys
import time
import uuid


REPO = Path(__file__).resolve().parents[1]


def sibling_module(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


harness = sibling_module("native_harness", "native-harness.py")
smoke = sibling_module("native_smoke", "native-smoke.py")


class WindowAttributes(C.Structure):
    _fields_ = [
        ("x", C.c_int), ("y", C.c_int), ("width", C.c_int), ("height", C.c_int),
        ("border_width", C.c_int), ("depth", C.c_int), ("visual", C.c_void_p),
        ("root", C.c_ulong), ("window_class", C.c_int), ("bit_gravity", C.c_int),
        ("win_gravity", C.c_int), ("backing_store", C.c_int),
        ("backing_planes", C.c_ulong), ("backing_pixel", C.c_ulong),
        ("save_under", C.c_int), ("colormap", C.c_ulong),
        ("map_installed", C.c_int), ("map_state", C.c_int),
        ("all_event_masks", C.c_long), ("your_event_mask", C.c_long),
        ("do_not_propagate_mask", C.c_long), ("override_redirect", C.c_int),
        ("screen", C.c_void_p),
    ]


class XError(C.Structure):
    _fields_ = [
        ("event_type", C.c_int), ("display", smoke.Display),
        ("resource_id", C.c_ulong), ("serial", C.c_ulong),
        ("error_code", C.c_ubyte), ("request_code", C.c_ubyte),
        ("minor_code", C.c_ubyte),
    ]


class OwnedX11(smoke.X11):
    def __init__(self, pid: int):
        super().__init__()
        self.pid = pid
        self.button_down = False
        self.motion_transform = None
        self.motion_calibration = None
        self.last_requested_native = None
        # Native close can destroy a queried window between XQueryTree and its
        # property read. Xlib's default handler exits Python without cleanup.
        self._declare("XSetErrorHandler", C.c_void_p, [C.c_void_p])
        self.x_errors = []
        handler_type = C.CFUNCTYPE(C.c_int, smoke.Display, C.POINTER(XError))
        def error_handler(_display, error):
            self.x_errors.append(error.contents.error_code)
            self.x_errors = self.x_errors[-16:]
            return 0
        self._error_handler = handler_type(error_handler)
        self._previous_handler = self.lib.XSetErrorHandler(C.cast(self._error_handler, C.c_void_p))
        self._declare("XGetWindowProperty", C.c_int, [
            smoke.Display, smoke.Window, C.c_ulong, C.c_long, C.c_long,
            C.c_int, C.c_ulong, C.POINTER(C.c_ulong), C.POINTER(C.c_int),
            C.POINTER(C.c_ulong), C.POINTER(C.c_ulong), C.POINTER(C.c_void_p),
        ])
        self._declare("XGetWindowAttributes", C.c_int, [
            smoke.Display, smoke.Window, C.POINTER(WindowAttributes),
        ])
        self._declare("XQueryPointer", C.c_int, [
            smoke.Display, smoke.Window, C.POINTER(smoke.Window), C.POINTER(smoke.Window),
            C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_int),
            C.POINTER(C.c_uint),
        ])
        self.pid_atom = self.lib.XInternAtom(self.display, b"_NET_WM_PID", 0)

    def window_pid(self, window: int) -> int | None:
        actual_type, count, remaining = C.c_ulong(), C.c_ulong(), C.c_ulong()
        actual_format, data = C.c_int(), C.c_void_p()
        result = self.lib.XGetWindowProperty(
            self.display, window, self.pid_atom, 0, 1, 0, 0,
            C.byref(actual_type), C.byref(actual_format), C.byref(count),
            C.byref(remaining), C.byref(data),
        )
        try:
            if result != 0 or actual_format.value != 32 or count.value != 1 or not data:
                return None
            return C.cast(data, C.POINTER(C.c_ulong))[0]
        finally:
            if data:
                self.lib.XFree(data)

    def windows(self) -> list[int]:
        pending, visited, found = [(self.root, 0)], set(), []
        while pending:
            window, depth = pending.pop()
            if window in visited:
                continue
            visited.add(window)
            if window != self.root and self.window_pid(window) == self.pid:
                attributes = WindowAttributes()
                if self.lib.XGetWindowAttributes(self.display, window, C.byref(attributes)):
                    if attributes.map_state == 2 and attributes.width >= 100 and attributes.height >= 100:
                        found.append(window)
            if depth < 5:
                pending.extend((child, depth + 1) for child in self.children(window))
        return sorted(found)

    def button(self, pressed: bool) -> None:
        if not self.input_library().XTestFakeButtonEvent(self.display, 1, int(pressed), 0):
            raise RuntimeError("XTest rejected primary-button input")
        self.button_down = pressed
        self.lib.XSync(self.display, 0)

    def move_in(self, window: int, x: int, y: int) -> None:
        origin = self.origin(window)
        self.move_to_native(origin[0] + x, origin[1] + y)

    def move_to_native(self, x: int, y: int) -> None:
        """Move to physical root coordinates through the measured XTest map."""
        if self.motion_transform is None:
            raise RuntimeError("Native pointer motion must be calibrated before input")
        gain_x, gain_y, offset_x, offset_y = self.motion_transform
        self.motion_absolute(round((x - offset_x) / gain_x),
                             round((y - offset_y) / gain_y))
        self.last_requested_native = (x, y)

    def motion_absolute(self, x: int, y: int) -> None:
        if not self.input_library().XTestFakeMotionEvent(self.display, -1, x, y, 0):
            raise RuntimeError("XTest rejected native pointer motion")
        self.lib.XSync(self.display, 0)

    def calibrate_motion(self, window: int, x: int, y: int, hint: float) -> dict:
        """Measure compositor XTest coordinates without pressing any button.

        XWayland's protocol and compositor stage coordinates can differ even
        when XQueryPointer reports a successful XWarpPointer. Measure XTest's
        actual pointer using two small moves, then verify the owned row target.
        Compositor input-seat startup may delay initial XTest motion; retries
        remain bounded and never press a button before calibration succeeds.
        """
        self.motion_transform = None
        report = {"status": "calibrating", "window": window, "seed_scale": hint, "attempts": []}
        self.motion_calibration = report
        if window not in self.windows() or self.button_down:
            report["status"] = "failed"
            raise RuntimeError("Calibration requires a mapped window of the owned PID with no primary input held")
        self.focus(window)
        time.sleep(0.3)
        for number in range(1, 5):
            origin = self.origin(window)
            target = [origin[0] + x, origin[1] + y]
            initial = [round(coordinate / hint) for coordinate in target]
            attempt = {"number": number, "origin_native": origin, "target_native": target,
                       "samples": [], "validation_samples": []}
            report["attempts"].append(attempt)
            # Warm up compositor delivery near the intended row without a
            # click. Preserve this observation even when the seat is not ready.
            warmup = [initial[0] - 12, initial[1] - 8]
            self.motion_absolute(*warmup)
            time.sleep(0.2)
            attempt["warmup"] = {"requested_xtest": warmup, "pointer": self.pointer(window)}
            for requested in (initial, [initial[0] + 20, initial[1] + 14]):
                self.motion_absolute(*requested)
                time.sleep(0.2)
                attempt["samples"].append({"requested_xtest": requested, "pointer": self.pointer(window)})
            before, after = [sample["pointer"]["root_position"] for sample in attempt["samples"]]
            gains = [(after[0] - before[0]) / 20, (after[1] - before[1]) / 14]
            attempt["gains"] = gains
            if all(math.isfinite(gain) and 0 < gain <= 8 for gain in gains):
                offsets = [before[axis] - initial[axis] * gains[axis] for axis in range(2)]
                attempt["offsets"] = offsets
                self.motion_transform = (*gains, *offsets)
                self.move_in(window, x, y)
                deadline = time.monotonic() + 0.8
                while time.monotonic() < deadline:
                    time.sleep(0.05)
                    observed = self.pointer(window)
                    attempt["validation_samples"].append(observed)
                    if observed["same_screen"] and all(abs(observed["root_position"][axis] - target[axis]) <= 2 for axis in range(2)):
                        report.update(status="passed", samples=attempt["samples"], gains=gains,
                                      offsets=offsets, target_native=target, validated_pointer=observed)
                        return report
                attempt["error"] = "Measured transform did not reach the owned row within two native pixels"
            else:
                attempt["error"] = "Motion samples did not establish finite positive gains"
            self.motion_transform = None
            if number < 4:
                self.focus(window)
                time.sleep(0.2)
        report["status"] = "failed"
        raise RuntimeError("Cannot validate XTest pointer transformation after four harmless-motion attempts; see pointer_calibration diagnostics")

    def click(self, window: int, x: int, y: int, button: int) -> None:
        if window not in self.windows():
            raise RuntimeError("Native click target is not a mapped window of the owned PID")
        width, height = self.geometry(window)
        if not (0 <= x < width and 0 <= y < height):
            raise RuntimeError(f"Click ({x}, {y}) is outside the {width}×{height} owned window")
        self.move_in(window, x, y)
        time.sleep(0.1)
        observed = self.pointer(window)
        if any(abs(observed["window_position"][axis] - point) > 2 for axis, point in enumerate((x, y))):
            raise RuntimeError(f"Native click pointer missed the owned target: {observed}")
        if button == 1:
            self.button(True)
            try:
                time.sleep(0.03)
            finally:
                self.button(False)
        else:
            if not self.input_library().XTestFakeButtonEvent(self.display, button, 1, 0):
                raise RuntimeError("XTest rejected native pointer press")
            self.lib.XSync(self.display, 0)
            try:
                time.sleep(0.03)
            finally:
                self.input_library().XTestFakeButtonEvent(self.display, button, 0, 0)
                self.lib.XSync(self.display, 0)

    def pointer(self, window: int) -> dict:
        root, child = smoke.Window(), smoke.Window()
        root_x, root_y, local_x, local_y = C.c_int(), C.c_int(), C.c_int(), C.c_int()
        mask = C.c_uint()
        same_screen = self.lib.XQueryPointer(
            self.display, window, C.byref(root), C.byref(child), C.byref(root_x), C.byref(root_y),
            C.byref(local_x), C.byref(local_y), C.byref(mask),
        )
        return {"same_screen": bool(same_screen), "root": root.value, "child": child.value,
                "root_position": [root_x.value, root_y.value],
                "window_position": [local_x.value, local_y.value], "button_mask": mask.value}

    def close(self) -> None:
        if self.display:
            self.lib.XSync(self.display, 0)
            super().close()
            self.lib.XSetErrorHandler(self._previous_handler)


def shell_identity(pid: int) -> tuple[int, str] | None:
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        if fields[0] == "Z":
            return None
        return int(fields[1]), fields[19]
    except (OSError, ValueError, IndexError):
        return None


def is_descendant(pid: int, parent: int) -> bool:
    for _ in range(32):
        if pid == parent:
            return True
        identity = shell_identity(pid)
        if identity is None or identity[0] <= 1:
            return False
        pid = identity[0]
    return False


def red_modal_button(path: Path, scale: float = 1.0) -> tuple[int, int] | None:
    """Find the unique large red button in the owned window's central sheet.

    The test fixes Graphite and excludes the red titlebar light. Never guess a
    destructive button's coordinates if the capture is missing or ambiguous.
    """
    from PIL import Image

    with Image.open(path) as image:
        pixels = image.convert("RGB")
        width, height = pixels.size
        region = (
            max(0, width // 2 - round(195 * scale)), max(round(35 * scale), height // 2 - round(150 * scale)),
            min(width, width // 2 + round(195 * scale)), min(height, height // 2 + round(150 * scale)),
        )
        red = set()
        for y in range(region[1], region[3]):
            for x in range(region[0], region[2]):
                r, g, b = pixels.getpixel((x, y))
                if r > 140 and g < r * 0.78 and b < r * 0.84:
                    red.add((x, y))
        buttons = []
        while red:
            start = red.pop()
            pending, component = [start], [start]
            while pending:
                x, y = pending.pop()
                for point in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]:
                    if point in red:
                        red.remove(point)
                        pending.append(point)
                        component.append(point)
            xs, ys = zip(*component)
            left, right, top, bottom = min(xs), max(xs), min(ys), max(ys)
            if len(component) >= 600 * scale * scale and 45 * scale <= right - left <= 160 * scale and 20 * scale <= bottom - top <= 50 * scale:
                buttons.append(((left + right) // 2, (top + bottom) // 2))
        if len(buttons) > 1:
            raise RuntimeError("The native close capture contains multiple possible destructive buttons")
        return buttons[0] if buttons else None


class Run:
    def __init__(self, args, output: Path, process: subprocess.Popen, address: str):
        self.args, self.output, self.process, self.address = args, output, process, address
        self.data = output / "data"
        self.x11 = OwnedX11(process.pid)
        self.scale = 1.0
        self.report = {
            "status": "running", "pid": process.pid, "address": address,
            "data_root": str(self.data), "platform": platform.platform(),
            "app": str(args.app), "client": str(args.client),
            "binary_sha256": hashlib.sha256(args.app.read_bytes()).hexdigest(),
            "lock_sha256": hashlib.sha256((REPO / "Cargo.lock").read_bytes()).hexdigest(),
            "features": ["inspection"], "input": "actual X11/XTest pointer and keyboard after bootstrap",
            "drag_to_native_point": args.drag_to_native_point,
            "drag_to_native_region": args.drag_to_native_region,
            "checks": [], "screenshots": [],
            "limitations": [
                "Wayland, macOS, Windows and mixed-DPI behavior are not verified.",
                ("Cross-monitor verification uses the native window center inside a caller-selected rectangle; monitor inventory and changes of scale are not verified."
                 if args.drag_to_native_point else "Cross-monitor dragging is not exercised without --drag-to-native-point."),
                "Logical UI coordinates use the bootstrap window scale; scale changes during the scenario are not verified.",
                "The inspection protocol has no viewport selector; it is used only while one window exists.",
                "History preservation requires visual review of the generated marker in before/after captures; terminal contents are not exposed by this harness.",
                "These native captures still require visual review; passing checks do not approve appearance.",
            ],
        }
        self.save()

    def save(self) -> None:
        (self.output / "workspace-windows.json").write_text(json.dumps(self.report, indent=2) + "\n")

    def wait(self, description, predicate, timeout=12):
        deadline, last_error = time.monotonic() + timeout, None
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"Owned Neptune exited during {description}: {self.process.returncode}")
            try:
                result = predicate()
                if result:
                    return result
            except (OSError, ValueError, RuntimeError, KeyError, IndexError) as error:
                last_error = str(error)
            time.sleep(0.05)
        raise RuntimeError(f"Timed out waiting for {description}; last error: {last_error}")

    def check(self, name, passed, detail):
        self.report["checks"].append({"name": name, "passed": bool(passed), "detail": detail})
        self.save()
        if not passed:
            raise RuntimeError(f"{name}: {detail}")
        print(f"PASS {name}", flush=True)

    def call(self, *arguments):
        # Keep generated terminal input out of the report and diagnostics.
        result = subprocess.run(
            [str(self.args.client), "--addr", self.address, *map(str, arguments)],
            text=True, capture_output=True, timeout=25,
        )
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or "Owned inspection call failed")
        return json.loads(result.stdout)

    def tree(self):
        tree = self.call("tree")["Tree"]["accesskit"]
        return [node for _, node in tree["nodes"]]

    def widget(self, label, role=None):
        def find():
            nodes = [n for n in self.tree() if n["properties"].get("label") == label and (role is None or n["role"] == role)]
            return nodes[0] if len(nodes) == 1 else None
        return self.wait(f"bootstrap widget {label}", find)

    @staticmethod
    def center(node):
        b = node["properties"]["bounds"]
        return round((b["x0"] + b["x1"]) / 2), round((b["y0"] + b["y1"]) / 2)

    def native_point(self, point):
        return tuple(round(coordinate * self.scale) for coordinate in point)

    def state(self):
        return json.loads((self.data / "workspaces.json").read_text())

    def workspace(self, name):
        return next((w for w in self.state()["workspaces"] if w["name"] == name), None)

    def count(self, count):
        return self.wait(f"{count} mapped native windows of exact PID {self.process.pid}", lambda: (windows if len(windows) == count else None) if (windows := self.x11.windows()) else None)

    def capture(self, name, window):
        if window not in self.x11.windows():
            raise RuntimeError(f"Capture {name} is not a mapped window of the owned PID")
        time.sleep(0.15)
        path = self.output / f"{name}.png"
        self.x11.snapshot(window, path)
        self.report["screenshots"].append({"name": name, "path": str(path), "window_id": window, "size": self.x11.geometry(window)})
        self.save()
        return path

    def bootstrap_rename(self, name):
        self.call("key", "ctrl+shift+p")
        self.call("text", "Rename workspace")
        self.call("key", "Enter")
        self.widget("Workspace rename", "textInput")
        self.call("key", "ctrl+a")
        self.call("text", name)
        self.call("key", "Enter")
        self.wait(f"saved workspace {name}", lambda: self.workspace(name))
        self.call("settle", 2)

    def bootstrap_shell(self, command):
        self.call("text", command)
        self.call("key", "Enter")

    def native_shell(self, window, command):
        self.x11.focus(window)
        width, height = self.x11.geometry(window)
        self.x11.click(window, width // 2, height // 2, 1)
        time.sleep(0.1)
        self.x11.type_text(command, 0.001)
        self.x11.chord("Return")

    def palette(self, window, query, capture=None, execute=False):
        self.x11.focus(window)
        self.x11.chord("ctrl+shift+p")
        time.sleep(0.25)
        self.x11.type_text(query, 0.02)
        # XSync flushes the X server, not compositor keyboard delivery. Allow
        # the complete query and key releases to reach the native application.
        time.sleep(0.5)
        if capture:
            self.capture(capture, window)
        self.x11.chord("Return" if execute else "Escape")
        time.sleep(0.3)

    def destination_menu(self, window, row):
        self.x11.focus(window)
        self.x11.click(window, *row, 3)
        self.capture("05a-detached-workspace-menu", window)
        # The fixed pointer context menu fits here. Its rows are 28px with a
        # 5px inner margin + 1px border: Rename, new window, existing window.
        # Only hover; never activate a coordinate-selected menu item.
        offset = self.native_point((111, 76))
        self.x11.move_in(window, row[0] + offset[0], row[1] + offset[1])
        time.sleep(0.2)
        self.capture("05b-existing-window-submenu", window)
        self.x11.chord("Escape")
        width, height = self.x11.geometry(window)
        self.x11.click(window, width // 2, height // 2, 1)
        time.sleep(0.15)

    def marker(self, label, token):
        pid = self.data / f"{label}.pid"
        command = f"export NEPTUNE_WINDOW_TOKEN={shlex.quote(token)}; printf '%s' \"$$\" > {shlex.quote(str(pid))}; printf 'NEPTUNE_HISTORY_{label.upper()}\\n'"
        return command, pid

    def verify_marker(self, window, label, before, token):
        pid_path, token_path = self.data / f"{label}-after.pid", self.data / f"{label}-after.token"
        self.native_shell(window, f"printf '%s' \"$$\" > {shlex.quote(str(pid_path))}; printf '%s' \"$NEPTUNE_WINDOW_TOKEN\" > {shlex.quote(str(token_path))}")
        self.wait(f"{label} live shell response", lambda: pid_path.read_text().isdecimal() and token_path.read_text() == token)
        after = int(pid_path.read_text())
        self.check(f"{label} keeps its live PTY", after == before[0] and shell_identity(after) == before[1] and token_path.read_text() == token,
                   "The same task-owned shell PID/start time and exported process state survive; native keyboard input still executes.")

    def shell_before(self, pid_path):
        self.wait("task shell PID marker", lambda: pid_path.exists() and pid_path.read_text().isdecimal())
        pid = int(pid_path.read_text())
        identity = shell_identity(pid)
        self.check("shell belongs to owned Neptune", identity is not None and is_descendant(pid, self.process.pid), "PID marker names a live descendant of this run's process.")
        return pid, identity

    def assignments(self, first, second):
        def assigned():
            snapshot = self.state()
            windows = [w for w in (snapshot.get("windows") or []) if w["workspaces"]]
            if len(windows) != 2:
                return None
            a = next((w for w in windows if first in w["workspaces"]), None)
            b = next((w for w in windows if second in w["workspaces"]), None)
            return windows if a and b and a["id"] != b["id"] else None
        return self.wait("separate durable workspace/window assignments", assigned)

    def drag_to_native_point(self, window, gesture, label, point):
        region = self.args.drag_to_native_region
        def sample():
            origin = self.x11.origin(window)
            size = self.x11.geometry(window)
            center = [origin[axis] + size[axis] / 2 for axis in range(2)]
            return {"origin_native": origin, "size_native": size, "center_native": center,
                    "center_in_region": region[0] <= center[0] < region[2] and region[1] <= center[1] < region[3],
                    "pointer": self.x11.pointer(window)}
        before = sample()
        restore_point = self.x11.last_requested_native
        if restore_point is None:
            raise RuntimeError("Cross-monitor dragging requires an earlier calibrated native pointer request")
        motion = {"requested_native": point, "target_region_native": region,
                  "before": before, "before_capture": f"{label}-held",
                  "last_requested_before_native": restore_point,
                  "verification": "Actual native window center inside caller-selected rectangle while primary input remains held; queried pointer position is descriptive during the window-manager grab."}
        gesture["cross_monitor"] = motion
        self.save()
        self.check(f"{label} starts outside caller-selected monitor region", not before["center_in_region"],
                   "The original pressed native window center is outside the supplied destination rectangle before cross-monitor motion.")
        self.x11.move_to_native(*point)
        def in_region():
            motion["last_motion_sample"] = sample()
            return motion["last_motion_sample"] if motion["last_motion_sample"]["center_in_region"] else None
        motion["after"] = self.wait("held native window center inside caller-selected monitor region", in_region)
        self.check(f"{label} reaches caller-selected monitor region", self.x11.button_down,
                   "The same pressed native window center enters the caller-selected physical monitor rectangle before release; actual native geometry establishes movement.")
        self.capture(f"{label}-cross-monitor-held", window)

        # Return to the adapter's previous request, rather than XQueryPointer's
        # potentially stale position during the WM grab. Observe window geometry
        # for acceptance and retain queried pointer samples as diagnostics.
        motion["restore_requested_native"] = restore_point
        self.x11.move_to_native(*restore_point)
        def outside_region():
            motion["last_restore_sample"] = sample()
            return motion["last_restore_sample"] if not motion["last_restore_sample"]["center_in_region"] else None
        motion["restored"] = self.wait("held native window center returns outside destination monitor region", outside_region)
        self.save()

    def drag(self, window, row, label, native_target=None):
        self.x11.focus(window)
        origin = self.x11.origin(window)
        self.x11.move_in(window, *row)
        time.sleep(0.1)
        gesture = {"name": label, "window": window, "size": self.x11.geometry(window),
                   "origin": origin, "row_native": row, "pointer_before_press": self.x11.pointer(window),
                   "steps": []}
        self.report.setdefault("gestures", []).append(gesture)
        self.save()
        self.x11.button(True)
        gesture["pointer_after_press"] = self.x11.pointer(window)
        self.save()
        try:
            target_x = round(285 * self.scale)
            to = (target_x, row[1])
            for step in range(1, 7):
                x = round(row[0] + (target_x - row[0]) * step / 6)
                self.x11.move_in(window, x, to[1])
                time.sleep(0.07)
                gesture["steps"].append({"requested": [x, to[1]], "pointer": self.x11.pointer(window),
                                         "origin": self.x11.origin(window)})
                self.save()
            windows = self.count(2)
            self.check(f"{label} spawns while pointer is held", self.x11.button_down and window in windows, "Actual X11 pointer press/motion created two mapped windows with the same task PID before release.")
            offset = self.native_point((105, 80))
            self.x11.move_in(window, to[0] + offset[0], to[1] + offset[1])
            moved = self.wait("pressed native source window follows pointer", lambda: (point if point != origin else None) if (point := self.x11.origin(window)) else None)
            self.check(f"{label} continues native window dragging", moved != origin, "The original pressed native window changed position while the primary button remained down.")
            self.capture(f"{label}-held", window)
            if native_target is not None:
                self.drag_to_native_point(window, gesture, label, native_target)
        finally:
            if self.x11.button_down:
                self.x11.button(False)
        return next(w for w in self.count(2) if w != window)

    def modal_button(self, window):
        path = self.output / "close-poll.png"
        def button():
            self.x11.snapshot(window, path)
            return red_modal_button(path, self.scale)
        return self.wait("native Close window destructive button", button)

    def scenario(self):
        self.call("settle", 2)
        root = self.count(1)[0]
        self.scale = self.call("tree")["Tree"]["pixels_per_point"]
        self.report["pixels_per_point"] = self.scale
        self.check("native coordinate scale measured", math.isfinite(self.scale) and self.scale > 0,
                   f"Bootstrap logical points use {self.scale} native pixels each; native input offsets and narrow dimensions use that uniform scale.")
        self.bootstrap_rename("Dragged")
        dragged_id = self.workspace("Dragged")["id"]
        token = uuid.uuid4().hex
        command, pid_path = self.marker("dragged", token)
        self.bootstrap_shell(command)
        dragged_shell = self.shell_before(pid_path)
        self.call("click", *self.center(self.widget("New workspace", "button")))
        self.wait("two bootstrap workspaces", lambda: len(self.state()["workspaces"]) == 2)
        self.bootstrap_rename("Sibling")
        sibling_id = self.workspace("Sibling")["id"]
        sibling_token = uuid.uuid4().hex
        command, pid_path = self.marker("sibling", sibling_token)
        self.bootstrap_shell(command)
        sibling_shell = self.shell_before(pid_path)
        row_node = self.widget("Dragged")
        row = self.center(row_node)
        native_row = self.native_point(row)
        close_node = self.widget("Close window", "button")
        close_control = self.native_point(self.center(close_node))
        self.report["bootstrap_coordinates"] = {
            "window": root, "size_native": self.x11.geometry(root), "origin_native": self.x11.origin(root),
            "row_role": row_node["role"], "row_bounds_logical": row_node["properties"]["bounds"],
            "row_center_logical": row, "row_center_native": native_row,
            "close_bounds_logical": close_node["properties"]["bounds"], "close_center_native": close_control,
        }
        self.save()
        self.call("click", *row)
        self.capture("01-before-detach", root)
        self.call("context", *row)
        self.widget("Move to new window")
        self.capture("02-workspace-menu", root)
        self.call("key", "Escape")
        self.call("key", "ctrl+shift+p")
        self.call("text", "Move workspace to new window")
        self.widget("Move workspace to new window")
        self.capture("03-detach-palette", root)
        self.call("key", "Escape")
        self.call("settle", 2)
        try:
            self.x11.calibrate_motion(root, *native_row, self.scale)
        finally:
            self.report["pointer_calibration"] = self.x11.motion_calibration
            self.save()

        sibling_native = self.drag(root, native_row, "04-root-detach", self.args.drag_to_native_point)
        self.assignments(dragged_id, sibling_id)
        self.verify_marker(root, "dragged", dragged_shell, token)
        self.capture("05-detached-workspace-history", root)
        self.destination_menu(root, native_row)
        self.capture("06-sibling-workspace", sibling_native)
        full_size = self.x11.geometry(root)
        narrow = self.native_point((640, 400))
        self.x11.lib.XResizeWindow(self.x11.display, root, *narrow)
        self.x11.lib.XSync(self.x11.display, 0)
        self.wait("640x400 logical detached window", lambda: self.x11.geometry(root) == narrow)
        self.capture("07-detached-narrow", root)
        self.palette(root, "Move workspace to Sibling", "08-rejoin-palette-narrow")
        self.x11.lib.XResizeWindow(self.x11.display, root, *full_size)
        self.x11.lib.XSync(self.x11.display, 0)
        self.wait("restored detached window size", lambda: self.x11.geometry(root) == full_size)
        self.palette(root, "Quit Neptune", "08a-quit-neptune-palette")

        self.x11.focus(root)
        self.x11.click(root, *close_control, 1)
        self.modal_button(root)
        self.capture("09-close-window-confirmation", root)
        self.x11.chord("Escape")
        time.sleep(0.2)
        self.check("cancel keeps both windows and sessions", len(self.count(2)) == 2 and shell_identity(dragged_shell[0]) == dragged_shell[1] and shell_identity(sibling_shell[0]) == sibling_shell[1], "Native Escape cancels the window close without replacing either shell.")
        self.x11.click(root, *close_control, 1)
        confirm = self.modal_button(root)
        self.capture("10-close-window-confirmation", root)
        self.x11.click(root, *confirm, 1)
        remaining = self.count(1)[0]
        self.wait("closed host workspace removed", lambda: self.workspace("Dragged") is None and len(self.state()["workspaces"]) == 1)
        self.check("closing native ROOT keeps sibling alive", remaining == sibling_native and self.process.poll() is None, "Only the confirmed window closes; the same-PID sibling remains interactive.")
        self.verify_marker(remaining, "sibling", sibling_shell, sibling_token)
        self.capture("11-sibling-after-root-close", remaining)

        # Exercise tear-off from a child after native ROOT has been hidden.
        self.x11.focus(remaining)
        self.x11.chord("ctrl+shift+n")
        self.wait("new workspace in surviving child", lambda: len(self.state()["workspaces"]) == 2)
        self.palette(remaining, "Rename workspace", execute=True)
        self.x11.chord("ctrl+a")
        self.x11.type_text("Rejoined", 0.001)
        self.x11.chord("Return")
        self.wait("child workspace renamed", lambda: self.workspace("Rejoined"))
        rejoined = self.workspace("Rejoined")["id"]
        child_token = uuid.uuid4().hex
        command, pid_path = self.marker("child", child_token)
        self.native_shell(remaining, command)
        child_shell = self.shell_before(pid_path)
        owner = next(w for w in self.state()["windows"] if rejoined in w["workspaces"])
        index = owner["workspaces"].index(rejoined)
        child_row = (native_row[0], native_row[1] + round(index * 42 * self.scale))
        replacement = self.drag(remaining, child_row, "12-child-detach")
        self.assignments(rejoined, sibling_id)
        self.verify_marker(remaining, "child", child_shell, child_token)
        self.palette(remaining, "Move workspace to Sibling", "13-rejoin-palette", execute=True)
        self.wait("rejoined durable memberships", lambda: any(rejoined in w["workspaces"] and sibling_id in w["workspaces"] for w in self.state()["windows"]))
        joined = self.count(1)[0]
        self.check("workspace rejoins existing native window", joined == replacement, "Palette movement reunites both stable workspace IDs and closes the emptied detached native child.")
        self.verify_marker(joined, "child-rejoined", child_shell, child_token)
        self.capture("14-rejoined-workspaces", joined)
        self.report["status"] = "passed"
        self.save()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--app", type=Path, default=REPO / "target/debug/neptune")
    parser.add_argument("--client", type=Path, default=REPO / "target/debug/neptune-inspect")
    parser.add_argument("--output", type=Path, default=REPO / "artifacts/native-workspace-windows")
    def native_point(value):
        try:
            x, y = value.split(",")
            return int(x), int(y)
        except (ValueError, TypeError):
            raise argparse.ArgumentTypeError("Use physical X11 desktop coordinates X,Y") from None
    parser.add_argument("--drag-to-native-point", type=native_point, metavar="X,Y",
                        help="During the first held tear-off, request motion to a physical desktop point on another monitor, capture the window, then return; pair with --drag-to-native-region")
    def native_region(value):
        try:
            left, top, right, bottom = map(int, value.split(","))
            if right <= left or bottom <= top:
                raise ValueError
            return left, top, right, bottom
        except (ValueError, TypeError):
            raise argparse.ArgumentTypeError("Use a nonempty physical monitor rectangle LEFT,TOP,RIGHT,BOTTOM") from None
    parser.add_argument("--drag-to-native-region", type=native_region, metavar="L,T,R,B",
                        help="Caller-selected destination monitor rectangle in physical X11 desktop coordinates; acceptance requires the held native window center inside it")
    args = parser.parse_args()
    if (args.drag_to_native_point is None) != (args.drag_to_native_region is None):
        parser.error("--drag-to-native-point and --drag-to-native-region must be supplied together")
    if not sys.platform.startswith("linux"):
        parser.error("This native adapter requires Linux/X11")
    args.app, args.client = args.app.resolve(), args.client.resolve()
    for binary in (args.app, args.client):
        if not binary.is_file():
            parser.error(f"Missing required inspection binary: {binary}")
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    output = args.output.resolve() / run_id
    data = output / "data"
    data.mkdir(parents=True)
    # Use a task-owned POSIX shell with no user history file or startup hooks.
    (data / "config.toml").write_text('shell = "/bin/sh"\nshell_args = ["-i"]\nconfirm_close = true\nwarn_running_processes = false\ncheck_updates = false\ndesktop_notifications = false\n')
    address = harness.free_endpoint()
    environment = os.environ.copy()
    environment.pop("WAYLAND_DISPLAY", None)
    environment["EGUI_INSPECTION"] = address
    run, process = None, None
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt("Owned native run stopped")))
    try:
        with (output / "app.log").open("w") as log:
            process = subprocess.Popen(
                [str(args.app), "--data-root", str(data), "--cwd", str(data), "--no-restore", "--size", "1180x760"],
                env=environment, stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
            )
            print(f"Native workspace-window run: {output}; owned PID {process.pid}; endpoint {address}", flush=True)
            harness.wait_ready(process, args.client, address, 30)
            run = Run(args, output, process, address)
            run.scenario()
            return 0
    except (Exception, KeyboardInterrupt) as error:
        if run is not None:
            run.report["status"], run.report["error"] = "failed", str(error)
            try:
                if run.x11.button_down:
                    run.x11.button(False)
                for window in run.x11.windows():
                    run.capture(f"failure-{window}", window)
            except Exception:
                pass
            run.save()
        else:
            (output / "workspace-windows.json").write_text(json.dumps({"status": "failed", "error": str(error), "address": address, "pid": process.pid if process else None}, indent=2) + "\n")
        print(f"Native workspace-window verification failed: {error}; artifacts: {output}", file=sys.stderr)
        return 1
    finally:
        if run is not None:
            try:
                if run.x11.button_down:
                    run.x11.button(False)
            finally:
                run.x11.close()
        if process is not None:
            harness.stop_owned(process)


if __name__ == "__main__":
    raise SystemExit(main())
