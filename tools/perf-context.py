#!/usr/bin/env python3
"""Record reproducible benchmark context without collecting credentials or full env."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess


def probe(command, cwd=None):
    try:
        result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=10)
        return {"returncode": result.returncode, "stdout": result.stdout.strip(), "stderr": result.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"error": str(error)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--window", default="1180x760 logical pixels (application default; verify on screen)")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Output already exists; choose a new output file.")
    binary = args.binary.resolve()
    with binary.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    root = Path(__file__).resolve().parent.parent
    cpu_model = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), None)
    graphics = probe(["lspci", "-nn"])
    if "stdout" in graphics:
        graphics["stdout"] = "\n".join(line for line in graphics["stdout"].splitlines() if any(kind in line for kind in ("VGA", "3D controller", "Display controller")))
    data = {
        "recorded_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": str(binary),
        "binary_sha256": digest,
        "binary_bytes": binary.stat().st_size,
        "git_revision_at_capture": probe(["git", "rev-parse", "HEAD"], cwd=root),
        "git_status_at_capture": probe(["git", "status", "--short"], cwd=root),
        "provenance_note": "The checkout revision at capture does not independently prove which revision produced a pre-existing binary.",
        "kernel": list(os.uname()),
        "cpu_model": cpu_model,
        "logical_cpu_count": os.cpu_count(),
        "memory_total": next((line for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemTotal:")), None),
        "rustc": probe(["rustc", "--version"]),
        "cargo": probe(["cargo", "--version"]),
        "python": platform.python_version(),
        "gpu_pci": graphics,
        "xvfb_available": shutil.which("Xvfb") is not None,
        "xrandr": probe(["xrandr", "--current"]),
        "glxinfo": probe(["glxinfo", "-B"]),
        "vulkaninfo": probe(["vulkaninfo", "--summary"]),
        "window": args.window,
        "environment": {key: os.environ.get(key) for key in ("DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "RUST_LOG", "WGPU_BACKEND")},
        "measurement_note": "Keep binary profile, window size/scaling, cache state, power settings, scene, logging, and background load fixed. xrandr describes Xwayland modes and may not reflect every Wayland surface. No GPU execution timings are measured by perf-sample.py.",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as output:
        json.dump(data, output, ensure_ascii=False, indent=2)
        output.write("\n")
    print(args.output.resolve())


if __name__ == "__main__":
    main()
