#!/usr/bin/env python3
"""Bounded Linux /proc sampler; no shell, third-party packages, or account access."""

import argparse
import csv
import datetime
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time


TICKS_PER_SECOND = os.sysconf("SC_CLK_TCK")
PAGE_BYTES = os.sysconf("SC_PAGE_SIZE")
IO_KEYS = ("rchar", "wchar", "read_bytes", "write_bytes")


def parse_stat(text):
    # comm may contain spaces and closing parentheses; fields follow its last ')'.
    fields = text.rsplit(")", 1)[1].split()
    return {
        "state": fields[0],
        "cpu_ticks": int(fields[11]) + int(fields[12]),
        "start_ticks": int(fields[19]),
        "rss_bytes": max(0, int(fields[21])) * PAGE_BYTES,
    }


def read_sample(pid):
    root = Path("/proc") / str(pid)
    sample = parse_stat((root / "stat").read_text())
    sample["time"] = time.monotonic()
    sample["io"] = None
    try:
        values = dict(line.split(":", 1) for line in (root / "io").read_text().splitlines())
        sample["io"] = {key: int(values[key]) for key in IO_KEYS}
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        pass  # Unavailable counters remain null, never a fabricated zero.
    sample["lifetime_peak_rss_bytes"] = None
    try:
        for line in (root / "status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                sample["lifetime_peak_rss_bytes"] = int(line.split()[1]) * 1024
                break
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        pass
    return sample


def percentiles(values):
    ordered = sorted(values)
    if not ordered:
        return {"p50": None, "p95": None, "p99": None}
    result = {}
    for label, fraction in (("p50", 0.50), ("p95", 0.95), ("p99", 0.99)):
        position = (len(ordered) - 1) * fraction
        lo, hi = math.floor(position), math.ceil(position)
        result[label] = ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo)
    return result


def summarize(samples):
    cpu_rates, intervals = [], []
    for previous, current in zip(samples, samples[1:]):
        elapsed = current["time"] - previous["time"]
        intervals.append(elapsed)
        cpu_rates.append((current["cpu_ticks"] - previous["cpu_ticks"]) / TICKS_PER_SECOND / elapsed * 100)
    span = samples[-1]["time"] - samples[0]["time"] if len(samples) > 1 else 0.0
    cpu_seconds = (samples[-1]["cpu_ticks"] - samples[0]["cpu_ticks"]) / TICKS_PER_SECOND if samples else 0.0
    io = None
    if len(samples) > 1 and all(item["io"] is not None for item in samples):
        io = {key: max(0, samples[-1]["io"][key] - samples[0]["io"][key]) for key in IO_KEYS}
    lifetime_peaks = [item["lifetime_peak_rss_bytes"] for item in samples if item["lifetime_peak_rss_bytes"] is not None]
    return {
        "sample_count": len(samples),
        "sample_span_seconds": span,
        "cpu_seconds_in_sample_span": cpu_seconds,
        "cpu_percent_one_core_mean": cpu_seconds / span * 100 if span else None,
        "cpu_percent_one_core_intervals": percentiles(cpu_rates),
        "sample_interval_seconds": percentiles(intervals),
        "rss_bytes": percentiles([item["rss_bytes"] for item in samples]),
        "peak_sampled_rss_bytes": max((item["rss_bytes"] for item in samples), default=None),
        "lifetime_peak_rss_bytes": max(lifetime_peaks, default=None),
        "io_bytes_in_sample_span": io,
    }


def stop_owned_process(child):
    # Only the new session created by this sampler is signalled. --pid is read-only.
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=2)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=2)


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, help="Observe an existing process; never signal it.")
    parser.add_argument("--output", type=Path, required=True, help="New output prefix (.json, .csv, and .child.log).")
    parser.add_argument("--duration", type=float, default=60, help="Measurement seconds, after warmup (default: 60).")
    parser.add_argument("--warmup", type=float, default=5, help="Excluded startup/settling seconds (default: 5).")
    parser.add_argument("--interval", type=float, default=0.25, help="Seconds between samples (default: 0.25).")
    parser.add_argument("--max-samples", type=int, default=36000, help="Hard memory/output bound (default: 36000).")
    parser.add_argument("--xdg-root", type=Path, help="Reuse a dedicated test XDG root; default is a fresh OUTPUT.xdg directory.")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="Command and arguments after --; never evaluated by a shell.")
    args = parser.parse_args(argv)
    if args.command[:1] == ["--"]:
        args.command = args.command[1:]
    if (args.pid is None) == (not args.command):
        parser.error("Specify exactly one of --pid PID or -- COMMAND [ARG ...].")
    if args.pid is not None and args.pid <= 0:
        parser.error("--pid must be positive.")
    if args.pid is not None and args.xdg_root is not None:
        parser.error("--xdg-root only applies when launching a command.")
    for name in ("duration", "warmup", "interval"):
        value = getattr(args, name)
        if not math.isfinite(value) or not 0 <= value <= 86400:
            parser.error(f"--{name} must be finite and between 0 and 86400 seconds.")
    if args.duration == 0 or args.interval < 0.01:
        parser.error("--duration must be positive and --interval must be at least 0.01 seconds.")
    if not 2 <= args.max_samples <= 1000000:
        parser.error("--max-samples must be between 2 and 1000000.")
    return args


def run(args):
    prefix = args.output.resolve()
    prefix.parent.mkdir(parents=True, exist_ok=True)
    json_path, csv_path = Path(str(prefix) + ".json"), Path(str(prefix) + ".csv")
    log_path = Path(str(prefix) + ".child.log")
    xdg_root = args.xdg_root.resolve() if args.xdg_root else Path(str(prefix) + ".xdg")
    outputs = [json_path, csv_path] + ([log_path] if args.command else [])
    if any(item.exists() for item in outputs):
        raise ValueError("Output already exists; choose a new --output prefix.")
    if args.command and args.xdg_root is None and xdg_root.exists():
        raise ValueError("Default isolated XDG directory already exists; choose a new --output prefix.")
    interrupted = threading.Event()
    old_handlers = {}
    for signum in (signal.SIGINT, signal.SIGTERM):
        old_handlers[signum] = signal.signal(signum, lambda _number, _frame: interrupted.set())
    samples = []
    child = None
    child_log = None
    reason = "error"
    error = None
    launch_time = time.monotonic()
    metadata = {
        "schema_version": 1,
        "recorded_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": list(os.uname()),
        "logical_cpu_count": os.cpu_count(),
        "clock_ticks_per_second": TICKS_PER_SECOND,
        "mode": "command" if args.command else "pid",
        "executable": args.command[0] if args.command else None,
        "requested_duration_seconds": args.duration,
        "requested_warmup_seconds": args.warmup,
        "requested_interval_seconds": args.interval,
        "max_samples": args.max_samples,
        "isolated_xdg_root": str(xdg_root) if args.command else None,
        "environment": {key: os.environ.get(key) for key in ("RUST_LOG", "WGPU_BACKEND", "DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE")},
        "notes": [
            "Only the selected process, including its threads, is measured; child processes and GPU execution are excluded.",
            "CPU 100% means one fully occupied logical core; percentages can exceed 100%.",
            "I/O deltas and CPU time cover only the span between first and last successful samples, not full process lifetime.",
            "rchar/wchar count syscall bytes (including cache); read_bytes/write_bytes count storage I/O charged by the kernel.",
            "RSS peaks may miss events between samples; lifetime VmHWM includes warmup and time before PID attachment.",
            "Null means unavailable. No environment secrets or full command argument list are recorded.",
        ],
    }
    try:
        if args.command:
            env = os.environ.copy()
            for key, name in (("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state")):
                directory = xdg_root / name
                directory.mkdir(parents=True, exist_ok=True)
                env[key] = str(directory)
            # This runner launches a binary, not its optional AppImage integration.
            env.pop("APPIMAGE", None)
            env.pop("APPDIR", None)
            child_log = log_path.open("x")
            child = subprocess.Popen(args.command, env=env, stdin=subprocess.DEVNULL, stdout=child_log, stderr=subprocess.STDOUT, start_new_session=True)
            pid = child.pid
        else:
            pid = args.pid
        metadata["pid"] = pid
        initial = read_sample(pid)
        metadata["process_start_ticks"] = initial["start_ticks"]
        warmup_end = time.monotonic() + args.warmup
        while time.monotonic() < warmup_end and not interrupted.is_set():
            if child is not None and child.poll() is not None:
                break
            interrupted.wait(min(0.1, max(0, warmup_end - time.monotonic())))
        measurement_start = time.monotonic()
        deadline = measurement_start + args.duration
        next_sample = measurement_start
        with csv_path.open("x", newline="") as stream:
            columns = ["elapsed_seconds", "cpu_ticks", "rss_bytes", "lifetime_peak_rss_bytes", *IO_KEYS]
            writer = csv.DictWriter(stream, fieldnames=columns)
            writer.writeheader()
            while True:
                if interrupted.is_set():
                    reason = "interrupted"
                    break
                try:
                    current = read_sample(pid)
                except (FileNotFoundError, ProcessLookupError):
                    reason = "process_exited"
                    break
                if current["start_ticks"] != initial["start_ticks"]:
                    reason = "pid_reused"
                    break
                if current["state"] in ("Z", "X"):
                    reason = "process_exited"
                    break
                samples.append(current)
                writer.writerow({
                    "elapsed_seconds": current["time"] - measurement_start,
                    "cpu_ticks": current["cpu_ticks"],
                    "rss_bytes": current["rss_bytes"],
                    "lifetime_peak_rss_bytes": current["lifetime_peak_rss_bytes"],
                    **(current["io"] or {key: None for key in IO_KEYS}),
                })
                if child is not None and child.poll() is not None:
                    reason = "process_exited"
                    break
                if len(samples) >= args.max_samples:
                    reason = "max_samples"
                    break
                if time.monotonic() >= deadline:
                    reason = "duration"
                    break
                # Skip missed deadlines instead of collecting a burst of artificial samples.
                next_sample += args.interval
                now = time.monotonic()
                if next_sample <= now:
                    next_sample = now + args.interval
                interrupted.wait(max(0, min(next_sample, deadline) - now))
    except (FileNotFoundError, ProcessLookupError) as ex:
        reason = "process_exited" if child is not None or args.pid is not None else "error"
        error = str(ex)
    except (OSError, ValueError) as ex:
        error = str(ex)
    finally:
        if child is not None:
            metadata["child_returncode_before_cleanup"] = child.poll()
            stop_owned_process(child)
            metadata["child_returncode_after_cleanup"] = child.returncode
        if child_log is not None:
            child_log.close()
        metadata.update({"stop_reason": reason, "error": error, "total_wall_seconds": time.monotonic() - launch_time, **summarize(samples)})
        with json_path.open("x") as stream:
            json.dump(metadata, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
        for signum, handler in old_handlers.items():
            signal.signal(signum, handler)
    print(json.dumps({"summary": str(json_path), "stop_reason": reason, **summarize(samples)}, ensure_ascii=False, indent=2))
    if reason == "interrupted":
        return 130
    if reason == "error" or error:
        return 1
    if metadata.get("child_returncode_before_cleanup") not in (None, 0):
        return 1
    return 0


def main():
    if not sys.platform.startswith("linux"):
        raise SystemExit("This tool requires Linux /proc.")
    try:
        return run(arguments())
    except (OSError, ValueError) as ex:
        print(f"perf-sample: {ex}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
