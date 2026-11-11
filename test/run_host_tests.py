"""Run portable HID parser/router tests with address/undefined-behaviour sanitizers."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
temp_root = Path("/tmp/opencode")
with tempfile.TemporaryDirectory(prefix="bt-switch-tests-", dir=temp_root if temp_root.exists() else None) as directory:
    for module in ("hid_parser", "hid_router"):
        binary = str(Path(directory) / f"{module}_test")
        subprocess.run([
            os.environ.get("CC", "cc"), "-std=c11", "-Wall", "-Wextra", "-Werror",
            "-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
            "-I", str(root / "src"), str(root / f"src/{module}.c"),
            str(root / f"test/{module}_test.c"), "-o", binary,
        ], check=True)
        subprocess.run([binary], check=True)
