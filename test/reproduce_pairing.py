"""Pair a second Linux host over SSH while collecting ESP UART + safe SMP metadata.

Does not remove bonds, power-cycle host adapters, or accept numeric comparison.
The scoped NoInputNoOutput agent accepts Just Works authorization for this device.
Run using PlatformIO's Python (pyserial); remote Python needs dbus + PyGObject.
"""
import argparse
from pathlib import Path
import re
import shlex
import subprocess
import threading
import time
import serial

REMOTE = r'''
import dbus
import dbus.service
import dbus.mainloop.glib
from gi.repository import GLib
import re
import subprocess
import threading

TARGET = "68:EE:8F:59:7F:36"
USE_AGENT = True
dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SystemBus()
loop = GLib.MainLoop()
success = False
pairing = False
discovering = False
device_path = None
safe_pattern = re.compile(r"SMP: Pairing (Request|Response|Failed)|IO capability:|Authentication requirement:|OOB data:|Encryption key size:|Initiator key distribution:|Responder key distribution:|Reason:|SMP: Security Request|Authentication Failed|Status: Authentication Failure")
monitor = subprocess.Popen(["sudo", "-n", "stdbuf", "-oL", "btmon"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
def monitor_reader():
    for line in monitor.stdout:
        if safe_pattern.search(line):
            print("SMP:", line.strip(), flush=True)
threading.Thread(target=monitor_reader, daemon=True).start()

class Rejected(dbus.DBusException):
    _dbus_error_name = "org.bluez.Error.Rejected"

class Agent(dbus.service.Object):
    @dbus.service.method("org.bluez.Agent1", in_signature="", out_signature="")
    def Release(self):
        pass
    @dbus.service.method("org.bluez.Agent1", in_signature="o", out_signature="")
    def RequestAuthorization(self, device):
        if str(device) != device_path:
            raise Rejected("Only the target test device is authorized")
        print("AGENT: Just Works authorization accepted for target", flush=True)
    @dbus.service.method("org.bluez.Agent1", in_signature="os", out_signature="")
    def AuthorizeService(self, device, uuid):
        if str(device) != device_path:
            raise Rejected("Not the target device")
    @dbus.service.method("org.bluez.Agent1", in_signature="ou", out_signature="")
    def RequestConfirmation(self, device, passkey):
        print("AGENT: unexpected numeric comparison requested; not auto-accepting", flush=True)
        raise Rejected("Test has no numeric comparison UI")
    @dbus.service.method("org.bluez.Agent1", in_signature="o", out_signature="u")
    def RequestPasskey(self, device):
        print("AGENT: unexpected passkey entry requested", flush=True)
        raise Rejected("No passkey input")
    @dbus.service.method("org.bluez.Agent1", in_signature="o", out_signature="s")
    def RequestPinCode(self, device):
        raise Rejected("Not a legacy PIN device")
    @dbus.service.method("org.bluez.Agent1", in_signature="", out_signature="")
    def Cancel(self):
        print("AGENT: cancelled", flush=True)

agent = Agent(bus, "/bt_switch_test_agent")
agent_manager = dbus.Interface(bus.get_object("org.bluez", "/org/bluez"), "org.bluez.AgentManager1")
if USE_AGENT:
    agent_manager.RegisterAgent("/bt_switch_test_agent", "NoInputNoOutput")
objects = dbus.Interface(bus.get_object("org.bluez", "/"), "org.freedesktop.DBus.ObjectManager")
adapter_path = next(str(p) for p, i in objects.GetManagedObjects().items() if "org.bluez.Adapter1" in i)
adapter = dbus.Interface(bus.get_object("org.bluez", adapter_path), "org.bluez.Adapter1")

def stop_scan():
    global discovering
    if discovering:
        adapter.StopDiscovery()
        discovering = False

def finish_pairing():
    global success
    props = dbus.Interface(bus.get_object("org.bluez", device_path), "org.freedesktop.DBus.Properties")
    paired = bool(props.Get("org.bluez.Device1", "Paired"))
    connected = bool(props.Get("org.bluez.Device1", "Connected"))
    success = paired and connected
    print(f"RESULT: paired={paired} connected={connected}", flush=True)
    loop.quit()
    return False

def paired():
    print("PAIR: completed", flush=True)
    GLib.timeout_add_seconds(3, finish_pairing)

def failed(error):
    print("PAIR FAILED:", error, flush=True)
    loop.quit()

def discover():
    global device_path, pairing
    if pairing:
        return False
    for path, interfaces in objects.GetManagedObjects().items():
        props = interfaces.get("org.bluez.Device1", {})
        if str(props.get("Address", "")).upper() == TARGET and "RSSI" in props:
            device_path = str(path)
            if props.get("Paired", False):
                print("SKIP: target already paired on second host; no bond removed", flush=True)
                loop.quit()
                return False
            pairing = True
            stop_scan()
            print("PAIR: attempting BLE Just Works", flush=True)
            device = dbus.Interface(bus.get_object("org.bluez", device_path), "org.bluez.Device1")
            device.Pair(reply_handler=paired, error_handler=failed, timeout=30)
            return False
    return True

def timed_out():
    print("FAIL: scan/pairing deadline exceeded", flush=True)
    loop.quit()
    return False

try:
    adapter.SetDiscoveryFilter(dbus.Dictionary({"Transport": "le"}, signature="sv"))
    adapter.StartDiscovery()
    discovering = True
    GLib.timeout_add(250, discover)
    GLib.timeout_add_seconds(40, timed_out)
    loop.run()
finally:
    stop_scan()
    if USE_AGENT:
        agent_manager.UnregisterAgent("/bt_switch_test_agent")
    monitor.terminate()
    try:
        monitor.wait(timeout=2)
    except subprocess.TimeoutExpired:
        monitor.kill()
raise SystemExit(0 if success else 1)
'''

parser = argparse.ArgumentParser()
parser.add_argument("--host", default="192.168.31.209")
parser.add_argument("--port", default="/dev/ttyUSB0")
parser.add_argument("--slot", type=int, choices=range(1, 7), default=2)
parser.add_argument("--agent-mode", choices=("noinput", "default"), default="noinput")
args = parser.parse_args()
port = serial.Serial(port=None, baudrate=115200, timeout=0.1)
port.dtr = False
port.rts = False
port.port = args.port
port.open()
uart = []
stop = threading.Event()
def collect():
    while not stop.is_set():
        uart.append(port.read(4096).decode("utf-8", errors="replace"))
thread = threading.Thread(target=collect)
thread.start()
try:
    # Opening some CH340 boards resets them. Wait for the serial command loop.
    time.sleep(2)
    port.write(f"{args.slot}\n".encode())
    time.sleep(1)
    port.write(b"p\ns\n")
    probe = REMOTE.replace("USE_AGENT = True", "USE_AGENT = " + str(args.agent_mode == "noinput"))
    command = ["/usr/bin/python3", "-c", probe] if args.host == "local" else [
        "ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", args.host,
        "python3 -c " + shlex.quote(probe),
    ]
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=50)
    print(result.stdout)
    port.write(b"s\n")
    time.sleep(2)
finally:
    stop.set()
    thread.join()
    port.close()

text = "".join(uart)
# No SMP keys or passkeys are emitted by this firmware. Redact any future UI
# passkey logs before persisting this diagnostic artifact.
text = re.sub(r"(?i)(passkey[:= ]+)\d+", r"\1<REDACTED>", text)
log = Path(f"/tmp/opencode/bluetooth-switch-pairing-{args.agent_mode}-uart.log")
log.write_text(text)
host_log = log.with_name(f"bluetooth-switch-pairing-{args.agent_mode}-host.log")
host_log.write_text(result.stdout)
for line in text.splitlines():
    if any(marker in line for marker in (
        "Authentication", "Paired host", "Selected slot=", "Slot ", "Host already",
        "Advertising slot", "Disconnected", "SEC_REQ", "NC_REQ", "Pairing policy",
    )):
        print("UART:", line)
print("UART log:", log)
if result.returncode or "Authentication failed:" in text:
    raise SystemExit("FAIL: second-host pairing failed")
if f"Slot {args.slot} ready" not in text:
    # A completed host pairing is insufficient if the bridge rejected that host.
    if not re.search(rf"Selected slot={args.slot} connected=1 ready=1", text):
        raise SystemExit("FAIL: second host did not reach firmware ready state")
print(f"PASS: host paired and ESP slot {args.slot} is ready")
