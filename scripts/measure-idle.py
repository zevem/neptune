#!/usr/bin/env python3
"""Observe one existing Linux process without sending input or signals.

/proc/<tgid>/stat aggregates CPU time across the process's threads, including
terminated threads. Child-process CPU and GPU work are outside this scope.
Whether the application is actually idle must be verified by the caller.

Wakeups are the voluntary context switches of each thread present at both ends of
the sample (a thread blocks once per wakeup), from /proc/<tgid>/task/*/status.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import platform
import statistics
import sys
import time


@dataclass(frozen=True)
class Snapshot:
    timestamp: float
    user_ticks: int
    system_ticks: int
    start_ticks: int
    rss_kib: int
    high_water_kib: int
    threads: int


def parse_stat(text: str) -> dict[str, int | str]:
    """Parse stat safely even when the parenthesized name contains spaces/')."""
    opening = text.find("(")
    closing = text.rfind(")")
    if opening < 0 or closing <= opening:
        raise ValueError("invalid /proc stat record")
    fields = text[closing + 1 :].split()
    if len(fields) < 22:
        raise ValueError("incomplete /proc stat record")
    # fields[0] is field 3 (state) in the Linux proc_pid_stat specification.
    return {
        "pid": int(text[:opening].strip()),
        "name": text[opening + 1 : closing],
        "state": fields[0],
        "user_ticks": int(fields[11]),
        "system_ticks": int(fields[12]),
        "threads": int(fields[17]),
        "start_ticks": int(fields[19]),
    }


def read_status(pid: int) -> dict[str, str]:
    return {
        name: value.strip()
        for line in Path(f"/proc/{pid}/status").read_text().splitlines()
        if ":" in line
        for name, value in [line.split(":", 1)]
    }


def status_kib(status: dict[str, str], field: str) -> int:
    value = status.get(field)
    if value is None:
        raise ValueError(f"{field} missing from process status")
    return int(value.split()[0])


def snapshot(pid: int) -> Snapshot:
    stat = parse_stat(Path(f"/proc/{pid}/stat").read_text())
    if stat["state"] == "Z":
        raise RuntimeError("process exited and is a zombie")
    status = read_status(pid)
    if int(status["Tgid"]) != pid:
        raise RuntimeError("expected a process/thread-group ID")
    return Snapshot(
        timestamp=time.monotonic(),
        user_ticks=int(stat["user_ticks"]),
        system_ticks=int(stat["system_ticks"]),
        start_ticks=int(stat["start_ticks"]),
        rss_kib=status_kib(status, "VmRSS"),
        high_water_kib=status_kib(status, "VmHWM"),
        threads=int(status["Threads"]),
    )


def thread_switches(pid: int) -> dict[int, tuple[str, int, int]]:
    """Thread name and voluntary/involuntary context switches, by thread ID."""
    threads = {}
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            status = read_status_path(task / "status")
        except OSError:
            continue
        threads[int(task.name)] = (
            status["Name"],
            int(status["voluntary_ctxt_switches"]),
            int(status["nonvoluntary_ctxt_switches"]),
        )
    return threads


def read_status_path(path: Path) -> dict[str, str]:
    return {
        name: value.strip()
        for line in path.read_text().splitlines()
        if ":" in line
        for name, value in [line.split(":", 1)]
    }


def wakeups(first: dict[int, tuple[str, int, int]], last: dict[int, tuple[str, int, int]], elapsed: float) -> dict:
    """Context switches per second, in total and by thread name, for threads alive throughout."""
    by_name: dict[str, float] = {}
    voluntary = involuntary = 0
    for tid, (name, vol, invol) in last.items():
        if tid not in first:
            continue
        delta = vol - first[tid][1]
        voluntary += delta
        involuntary += invol - first[tid][2]
        by_name[name] = by_name.get(name, 0) + delta
    return {
        "scope": "voluntary switches of threads alive at both ends of the sample",
        "voluntary_per_second": round(voluntary / elapsed, 3),
        "involuntary_per_second": round(involuntary / elapsed, 3),
        "threads_compared": sum(1 for tid in last if tid in first),
        "threads_started_or_ended": len(set(first) ^ set(last)),
        "voluntary_per_second_by_thread_name": {
            name: round(count / elapsed, 3) for name, count in sorted(by_name.items(), key=lambda item: -item[1])
        },
    }


def finite_duration(value: str) -> float:
    duration = float(value)
    if not math.isfinite(duration) or duration < 0:
        raise argparse.ArgumentTypeError("must be a finite, nonnegative number")
    return duration


def positive_duration(value: str) -> float:
    duration = finite_duration(value)
    if duration == 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return duration


def positive_pid(value: str) -> int:
    pid = int(value)
    if pid <= 0:
        raise argparse.ArgumentTypeError("PID must be positive")
    return pid


def cpu_model() -> str:
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.machine()


def measure(args: argparse.Namespace) -> dict:
    # A caller may supply a thread ID. Normalize to the whole process so that
    # worker-thread CPU is never accidentally excluded from the measurement.
    pid = int(read_status(args.pid)["Tgid"])
    initial = snapshot(pid)
    identity = initial.start_ticks
    try:
        # Preserve Linux's " (deleted)" suffix when a running build has been
        # replaced on disk; that is useful evidence of the measured binary.
        executable = os.readlink(f"/proc/{pid}/exe")
    except OSError:
        executable = None

    time.sleep(args.warmup)
    first_threads = thread_switches(pid)
    first = snapshot(pid)
    if first.start_ticks != identity:
        raise RuntimeError("process identity changed during warmup (PID reused)")
    samples = [first]
    deadline = first.timestamp + args.duration
    while time.monotonic() < deadline:
        remaining = deadline - time.monotonic()
        time.sleep(max(0.0, min(args.interval, remaining)))
        current = snapshot(pid)
        if current.start_ticks != identity:
            raise RuntimeError("process identity changed during sampling (PID reused)")
        samples.append(current)

    last_threads = thread_switches(pid)
    last = samples[-1]
    elapsed = last.timestamp - first.timestamp
    if elapsed <= 0:
        raise RuntimeError("sample duration was too short")
    ticks_per_second = os.sysconf("SC_CLK_TCK")
    user_ticks = last.user_ticks - first.user_ticks
    system_ticks = last.system_ticks - first.system_ticks
    if user_ticks < 0 or system_ticks < 0:
        raise RuntimeError("process CPU counters decreased")
    cpu_seconds = (user_ticks + system_ticks) / ticks_per_second
    rss = [sample.rss_kib for sample in samples]
    return {
        "schema_version": 1,
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "label": args.label,
        "idle_verified": False,
        "activity_verification": "Caller must verify no input, output, resize, or UI automation during sampling.",
        "requested_pid": args.pid,
        "pid": pid,
        "executable": executable,
        "process_start_ticks": identity,
        "warmup_seconds": args.warmup,
        "sample_seconds_requested": args.duration,
        "sample_seconds_actual": round(elapsed, 6),
        "sample_interval_seconds": args.interval,
        "sample_count": len(samples),
        "cpu": {
            "scope": "whole process, all threads; excludes child processes and GPU",
            "ticks_per_second": ticks_per_second,
            "user_ticks_delta": user_ticks,
            "system_ticks_delta": system_ticks,
            "seconds": round(cpu_seconds, 6),
            "percent_of_one_core": round(100 * cpu_seconds / elapsed, 4),
            "resolution_seconds": 1 / ticks_per_second,
        },
        "wakeups": wakeups(first_threads, last_threads, elapsed),
        "memory": {
            "rss_start_kib": rss[0],
            "rss_end_kib": rss[-1],
            "rss_min_sampled_kib": min(rss),
            "rss_max_sampled_kib": max(rss),
            "rss_mean_sampled_kib": round(statistics.mean(rss), 2),
            "lifetime_high_water_end_kib": last.high_water_kib,
        },
        "threads": {
            "start": first.threads,
            "end": last.threads,
            "max_sampled": max(sample.threads for sample in samples),
        },
        "host": {
            "os": platform.platform(),
            "architecture": platform.machine(),
            "cpu_model": cpu_model(),
            "logical_cpus": os.cpu_count(),
            "python_version": platform.python_version(),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=positive_pid, help="existing application process ID")
    parser.add_argument("--warmup", type=finite_duration, default=5.0, help="seconds before measurement (default: 5)")
    parser.add_argument("--duration", type=positive_duration, default=5.0, help="seconds sampled (default: 5)")
    parser.add_argument("--interval", type=positive_duration, default=0.25, help="RSS sampling interval (default: 0.25)")
    parser.add_argument("--label", default="unverified process observation", help="describe build and test conditions")
    parser.add_argument("--output", type=Path, help="optional JSON file; JSON is always printed to stdout")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("requires Linux /proc")
    try:
        result = measure(args)
        encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
        if args.output is not None:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded)
        sys.stdout.write(encoded)
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({"error": str(error), "requested_pid": args.pid}), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
