"""Non-destructive UART smoke test; run with PlatformIO's Python (pyserial)."""
import argparse
from pathlib import Path
import time
import serial

parser = argparse.ArgumentParser()
parser.add_argument("--port", default="/dev/ttyUSB0")
parser.add_argument("--seconds", type=float, default=6)
args = parser.parse_args()

port = serial.Serial(port=None, baudrate=115200, timeout=0.1)
port.dtr = False
port.rts = False
port.port = args.port
port.open()
chunks = []
start = time.monotonic()
next_status = start
try:
    while time.monotonic() - start < args.seconds:
        if time.monotonic() >= next_status:
            port.write(b"s\n")
            next_status += 1
        chunks.append(port.read(4096))
finally:
    port.close()
text = b"".join(chunks).decode("utf-8", errors="replace")
log = Path("/tmp/opencode/bluetooth-switch-boot.log")
if log.parent.is_dir():
    log.write_text(text)
    print(f"UART log: {log}")
panic = any(marker in text for marker in ("abort()", "Guru Meditation", "ESP_ERROR_CHECK failed", "Rebooting..."))
responsive = "Selected slot=" in text and "Active HID interfaces=" in text
for line in text.splitlines():
    if any(marker in line for marker in ("ESP_ERROR_CHECK", "expression:", "Selected slot=", "Active HID interfaces=", "Advertising slot")):
        print(line)
if panic:
    raise SystemExit("FAIL: firmware panic/reboot observed")
if not responsive:
    raise SystemExit("FAIL: no application status response received")
print("PASS: application responds to UART status commands; no panic observed")
