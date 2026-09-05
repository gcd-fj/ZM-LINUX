#!/usr/bin/env python3
"""Rebuild the AVM2 regression SWF using the same ASC inputs as build-bridges.sh."""
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
fixture = root / "crates/zm-player/tests/fixtures"
with tempfile.TemporaryDirectory() as directory:
    source = Path(directory) / "JsonNumbers.as"
    shutil.copyfile(fixture / source.name, source)
    subprocess.run([
        "java", "-jar", os.environ["RUFFLE_ASC_JAR"], "-AS3", "-import",
        os.environ["RUFFLE_PLAYERGLOBAL"], str(source),
    ], check=True)
    abc = source.with_suffix(".abc").read_bytes()


def tag(code, data):
    return struct.pack("<HI", (code << 6) | 63, len(data)) + data


body = b"\x08\x00" + struct.pack("<HH", 30 * 256, 1)
body += tag(69, struct.pack("<I", 8))  # ActionScript 3
body += tag(82, b"\0" * 4 + b"json-test\0" + abc)
body += tag(76, struct.pack("<HH", 1, 0) + b"JsonNumbers\0")
body += tag(1, b"") + tag(0, b"")
# JSON is available from Flash Player 11 / SWF version 13 onward.
(fixture / "JsonNumbers.swf").write_bytes(b"FWS\x14" + struct.pack("<I", 8 + len(body)) + body)
