#!/usr/bin/env python3
"""Measure retained native memory after output saturation and folder browsing.

Linux/X11 only. Each repetition owns its process, endpoint and fresh storage.
RSS includes shared mappings; PSS/private bytes exclude more shared memory, but
none of these measure GPU memory or the shell's memory. Fixture contents are
synthetic. Build the two release inspection binaries before running.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shlex
import subprocess
import time
import uuid

REPO = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("memory_scale", REPO / "scripts/measure-scale.py")
scale = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(scale)
harness = scale.harness


def memory(pid):
    status = Path(f"/proc/{pid}/status").read_text()
    smaps = Path(f"/proc/{pid}/smaps_rollup").read_text()
    fields = {}
    for line in (status + smaps).splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[-1] == "kB":
            fields[parts[0].rstrip(":")] = int(parts[1]) * 1024
    return {
        "rss_bytes": fields["VmRSS"],
        "pss_bytes": fields["Pss"],
        "private_bytes": fields["Private_Clean"] + fields["Private_Dirty"],
        "peak_rss_bytes": fields["VmHWM"],
    }


def node(client, address, label):
    return next((n for n in scale.tree_nodes(client, address)
                 if n["properties"].get("label") == label), None)


def click_label(client, address, label):
    found = scale.until(lambda: node(client, address, label), 15, label)
    scale.click(client, address, found)


def fixture(root, folders, entries):
    root.mkdir(parents=True)
    for index in range(folders):
        folder = root / f"folder-{index:02}"
        folder.mkdir()
        for item in range(entries):
            (folder / f"item-{item:05}-synthetic-memory-audit-fixture.txt").touch()


def run(args, output, scenario, source):
    output.mkdir(parents=True)
    data = output / "data"
    data.mkdir()
    (data / "config.toml").write_text(
        'shell="/bin/sh"\nscrollback=10000\ncursor_blink=false\n'
        'confirm_close=false\nwarn_running_processes=false\n'
    )
    address = harness.free_endpoint()
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["EGUI_INSPECTION"] = address
    log = output / "app.log"
    report = {"scenario": scenario, "status": "running", "endpoint": address,
              "data_root": str(data), "samples": []}
    process = None
    started = time.monotonic()

    def sample(label):
        time.sleep(args.settle)
        readings = []
        for _ in range(5):
            readings.append(memory(process.pid))
            time.sleep(0.1)
        result = {"label": label, "elapsed_seconds": time.monotonic() - started,
                  "readings": readings}
        for field in readings[0]:
            result[field] = sorted(r[field] for r in readings)[2]
        events = scale.frames(log)
        if events:
            result["parsed_bytes"] = sum(p["parsed"] for p in events[-1]["panes"])
            result["cache_estimated_bytes"] = events[-1]["cache_estimated_bytes"]
        report["samples"].append(result)
        print(f"{output.name} {label}: RSS {result['rss_bytes'] / 2**20:.1f} MiB; "
              f"private {result['private_bytes'] / 2**20:.1f} MiB", flush=True)

    try:
        with log.open("w") as sink:
            process = subprocess.Popen(
                [str(args.app), "--data-root", str(data), "--cwd", str(source),
                 "--no-restore", "--diagnostics", "--size", "1180x760"],
                env=env, stdout=sink, stderr=subprocess.STDOUT, start_new_session=True,
            )
            report["pid"] = process.pid
            harness.wait_ready(process, args.client, address, 40)
            scale.until(lambda: scale.frames(log) and
                        scale.frames(log)[-1]["resources"]["running"] == 1, 30, "shell")
            time.sleep(3)
            if scenario == "terminal":
                ready = data / "shell-ready"
                scale.command(args.client, address,
                              f"stty -echo -onlcr; touch {shlex.quote(str(ready))}")
                scale.until(ready.exists, 10, "exact-byte PTY setup")
            sample("quiet")
            if scenario in ("explorer", "explorer-close"):
                click_label(args.client, address, "Toggle right panel")
                # The tab strip slides in. Its initial accessibility bounds
                # describe the animation's starting position.
                time.sleep(0.5)
                click_label(args.client, address, "Files")
                visits = args.folders if scenario == "explorer" else 1
                for index in range(visits):
                    label = f"folder-{index:02}"
                    click_label(args.client, address, label)
                    scale.until(lambda: node(args.client, address,
                                "item-00000-synthetic-memory-audit-fixture.txt"), 20, "folder loaded")
                    if index == 0:
                        harness.inspect(args.client, address, "screenshot", str(output / "explorer-open.png"))
                    if scenario == "explorer":
                        click_label(args.client, address, label)
                        scale.until(lambda: not node(args.client, address,
                                    "item-00000-synthetic-memory-audit-fixture.txt"), 20, "folder collapsed")
                        if (index + 1) % 4 == 0:
                            sample(f"browsed-{index + 1}")
                if scenario == "explorer":
                    harness.inspect(args.client, address, "screenshot", str(output / "explorer-collapsed.png"))
                else:
                    sample("expanded")
                click_label(args.client, address, "Toggle right panel")
                scale.until(lambda: not node(args.client, address, "Files"), 15, "panel hidden")
                report["panel_hidden_verified"] = True
                sample("panel-closed")
                harness.inspect(args.client, address, "screenshot", str(output / "explorer-closed.png"))
                click_label(args.client, address, "Toggle right panel")
                time.sleep(0.5)
                if scenario == "explorer":
                    click_label(args.client, address, "folder-00")
                scale.until(lambda: node(args.client, address,
                            "item-00000-synthetic-memory-audit-fixture.txt"), 20, "folder reopened")
                harness.inspect(args.client, address, "resize", "640", "400")
                time.sleep(0.5)
                harness.inspect(args.client, address, "screenshot", str(output / "explorer-narrow.png"))
            else:
                for batch in range(6):
                    marker = data / f"output-{batch}.done"
                    producer_started = data / f"output-{batch}.started"
                    program = (
                        "import os,pathlib;"
                        f"pathlib.Path({str(producer_started)!r}).touch();"
                        f"data=b''.join((f'memory-audit {batch} {{i:06}} '.encode()+b'x'*100+b'\\r\\n') "
                        "for i in range(30000));"
                        "view=memoryview(data);"
                        "\nwhile view:\n view=view[os.write(1,view):]\n"
                        f"pathlib.Path({str(marker)!r}).touch()"
                    )
                    prior = scale.frames(log)[-1]["panes"][0]["parsed"]
                    expected = 30000 * len(f"memory-audit {batch} 000000 ".encode() + b"x" * 100 + b"\r\n")
                    report["output_bytes_per_batch"] = expected
                    scale.command(args.client, address, "python3 -c " + shlex.quote(program))
                    scale.until(producer_started.exists, 10, "producer started")
                    scale.until(marker.exists, 30, "producer finished")
                    scale.until(lambda: any(p["parsed"] >= prior + expected and
                                p["parsed"] == p["received"] for p in scale.frames(log)[-1]["panes"]),
                                30, "all output parsed")
                    sample(f"output-{batch + 1}")
                harness.inspect(args.client, address, "screenshot", str(output / "terminal.png"))
            click_label(args.client, address, "Close window")
            process.wait(timeout=10)
            shutdowns = [json.loads(line) for line in log.read_text().splitlines()
                         if line.startswith("{") and '"operation":"shutdown"' in line]
            report["shutdown_complete"] = bool(shutdowns and shutdowns[-1]["complete"])
            if not report["shutdown_complete"]:
                raise RuntimeError("Incomplete session teardown")
            report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=str(error))
        print(f"{output.name}: {error}", flush=True)
        if process is not None and process.poll() is None:
            try:
                harness.inspect(args.client, address, "screenshot", str(output / "failed.png"))
            except (OSError, RuntimeError, subprocess.TimeoutExpired):
                pass
    finally:
        if process is not None:
            harness.stop_owned(process)
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=REPO / "target/release/neptune")
    parser.add_argument("--client", type=Path, default=REPO / "target/release/neptune-inspect")
    parser.add_argument("--output", type=Path, default=REPO / "artifacts/memory")
    parser.add_argument("--scenarios", nargs="+", choices=["terminal", "explorer", "explorer-close"],
                        default=["terminal", "explorer"])
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--folders", type=int, default=20)
    parser.add_argument("--entries", type=int, default=10000)
    parser.add_argument("--settle", type=float, default=1)
    parser.add_argument("--fixture", type=Path, help="Reuse a synthetic fixture from an earlier run")
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("Linux /proc and an X11 display are required")
    if not 1 <= args.repeats <= 10 or not 4 <= args.folders <= 20 or not 1 <= args.entries <= 10000:
        parser.error("Use 1..10 repeats, 4..20 folders and 1..10000 entries")
    if not 0.1 <= args.settle <= 10:
        parser.error("Use 0.1..10 settle seconds")
    args.app = args.app.resolve()
    args.client = args.client.resolve()
    if not args.app.is_file() or not args.client.is_file():
        parser.error("Build both release inspection binaries first")
    ident = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    root = args.output.resolve() / ident
    root.mkdir(parents=True)
    source = args.fixture.resolve() if args.fixture else root / "fixture"
    if args.fixture:
        if not source.is_dir():
            parser.error("The reused fixture must be a directory")
    elif any(scenario.startswith("explorer") for scenario in args.scenarios):
        fixture(source, args.folders, args.entries)
    else:
        source.mkdir()
    report = {
        "platform": platform.platform(), "features": ["inspection"], "display": "X11",
        "compiler": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "binary_sha256": hashlib.sha256(args.app.read_bytes()).hexdigest(),
        "lock_sha256": hashlib.sha256((REPO / "Cargo.lock").read_bytes()).hexdigest(),
        "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "folders": args.folders, "entries_per_folder": args.entries, "cases": [],
    }
    for repeat in range(args.repeats):
        for scenario in args.scenarios:
            report["cases"].append(run(args, root / f"{repeat + 1}-{scenario}", scenario, source))
            (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Measurements: {root / 'report.json'}", flush=True)
    return int(any(case["status"] != "passed" for case in report["cases"]))


if __name__ == "__main__":
    raise SystemExit(main())
