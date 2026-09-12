#!/usr/bin/env python3
"""Deterministic unit tests plus small local-process sampler integration tests."""

import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest


SCRIPT = Path(__file__).with_name("perf-sample.py")
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("perf_sample", SCRIPT)
PERF = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PERF)


class SamplerTests(unittest.TestCase):
    def test_stat_with_spaces_and_parentheses(self):
        fields = ["S"] + ["0"] * 21
        fields[11], fields[12], fields[19], fields[21] = "14", "6", "73", "100"
        sample = PERF.parse_stat("123 (a ) strange name)) " + " ".join(fields))
        self.assertEqual(sample["cpu_ticks"], 20)
        self.assertEqual(sample["start_ticks"], 73)
        self.assertEqual(sample["rss_bytes"], 100 * PERF.PAGE_BYTES)

    def test_summary_uses_sample_span_and_one_core(self):
        base = {"rss_bytes": 4096, "lifetime_peak_rss_bytes": 8192, "io": dict.fromkeys(PERF.IO_KEYS, 100)}
        samples = [dict(base, time=1.0, cpu_ticks=0), dict(base, time=3.0, cpu_ticks=PERF.TICKS_PER_SECOND * 4)]
        result = PERF.summarize(samples)
        self.assertEqual(result["cpu_percent_one_core_mean"], 200.0)
        self.assertEqual(result["io_bytes_in_sample_span"], dict.fromkeys(PERF.IO_KEYS, 0))
        samples[-1]["io"] = None
        self.assertIsNone(PERF.summarize(samples)["io_bytes_in_sample_span"])

    def test_empty_summary_does_not_invent_measurements(self):
        result = PERF.summarize([])
        self.assertIsNone(result["cpu_percent_one_core_mean"])
        self.assertIsNone(result["peak_sampled_rss_bytes"])
        self.assertEqual(result["rss_bytes"], dict.fromkeys(("p50", "p95", "p99")))

    def command(self, prefix, code, *options):
        return [sys.executable, str(SCRIPT), "--output", str(prefix), "--warmup", "0", "--interval", "0.02", "--duration", "0.2", *options, "--", sys.executable, "-c", code]

    def test_command_isolation_duration_and_literal_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / "duration"
            code = "import os,time; print(os.environ['XDG_CONFIG_HOME'], flush=True); print('$(exit 19)', flush=True); time.sleep(10)"
            result = subprocess.run(self.command(prefix, code), capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = json.loads(prefix.with_suffix(".json").read_text())
            self.assertEqual(summary["stop_reason"], "duration")
            self.assertGreaterEqual(summary["sample_count"], 2)
            self.assertEqual(summary["child_returncode_after_cleanup"], -signal.SIGTERM)
            log = prefix.with_suffix(".child.log").read_text()
            self.assertIn(str(prefix) + ".xdg/config", log)
            self.assertIn("$(exit 19)", log)
            self.assertNotIn("time.sleep", json.dumps(summary))

    def test_exit_during_warmup_and_missing_command(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / "exit"
            result = subprocess.run(self.command(prefix, "raise SystemExit(7)", "--warmup", "0.2"), capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 1)
            summary = json.loads(prefix.with_suffix(".json").read_text())
            self.assertEqual(summary["stop_reason"], "process_exited")
            self.assertEqual(summary["sample_count"], 0)
            missing = Path(directory) / "missing"
            result = subprocess.run([sys.executable, str(SCRIPT), "--output", str(missing), "--", "/does-not-exist/perf-test"], capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 1)
            self.assertEqual(json.loads(missing.with_suffix(".json").read_text())["stop_reason"], "error")

    def test_max_samples_is_a_hard_bound(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / "bounded"
            result = subprocess.run(self.command(prefix, "import time; time.sleep(10)", "--max-samples", "2"), capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = json.loads(prefix.with_suffix(".json").read_text())
            self.assertEqual(summary["stop_reason"], "max_samples")
            self.assertEqual(summary["sample_count"], 2)
            self.assertEqual(len(prefix.with_suffix(".csv").read_text().splitlines()), 3)

    def test_pid_mode_never_terminates_target(self):
        target = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"])
        try:
            with tempfile.TemporaryDirectory() as directory:
                prefix = Path(directory) / "attach"
                result = subprocess.run([sys.executable, str(SCRIPT), "--output", str(prefix), "--pid", str(target.pid), "--duration", "0.1", "--warmup", "0"], capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIsNone(target.poll())
                self.assertFalse(prefix.with_suffix(".xdg").exists())
        finally:
            target.terminate()
            target.wait(timeout=3)

    def test_sigint_writes_summary_and_cleans_up_owned_child(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / "interrupt"
            sampler = subprocess.Popen(self.command(prefix, "import time; time.sleep(10)", "--duration", "10"), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 3
                while not prefix.with_suffix(".csv").exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(prefix.with_suffix(".csv").exists())
                sampler.send_signal(signal.SIGINT)
                _stdout, stderr = sampler.communicate(timeout=5)
                self.assertEqual(sampler.returncode, 130, stderr)
                summary = json.loads(prefix.with_suffix(".json").read_text())
                self.assertEqual(summary["stop_reason"], "interrupted")
                with self.assertRaises(ProcessLookupError):
                    os.kill(summary["pid"], 0)
            finally:
                if sampler.poll() is None:
                    sampler.kill()
                    sampler.wait(timeout=3)

    def test_rejects_unbounded_or_invalid_parameters(self):
        for options in (["--duration", "nan"], ["--interval", "0"], ["--max-samples", "1"], ["--duration", "inf"]):
            with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
                PERF.arguments(["--output", "unused", "--pid", "1", *options])


if __name__ == "__main__":
    unittest.main()
