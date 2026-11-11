"""Hardware regression: two bonded hosts stay connected while routing changes.

Sends only all-released reports (serial `t`), never keys or mouse motion. Assumes
slots 1/2 are paired and online, slot 3 is empty, and serial monitor is closed.
The SSH host is slot 2. Optional btmon observes only HID notification counts;
encryption keys, passkeys and report contents are not captured or printed.
Finally disconnects/reconnects only host 2's target device to test recovery;
use --skip-reconnect to omit that step. Never removes pairing records.
"""
import argparse
from pathlib import Path
import re
import selectors
import shlex
import subprocess
import time
import serial

MONITOR = r'''
import re,subprocess,threading,time
p=subprocess.Popen(["sudo","-n","stdbuf","-oL","btmon"],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
count=0
def collect():
    global count
    pending=False
    for line in p.stdout:
        if "ATT: Handle Value Notification" in line:
            pending=True
        elif pending and "Handle:" in line:
            if re.search(r"Handle: 0x(0040|004c|0053)\b",line,re.I):
                count+=1
            pending=False
t=threading.Thread(target=collect,daemon=True);t.start()
time.sleep(.3)
if p.poll() is not None:
    raise SystemExit("ERROR: btmon could not start")
print("READY",flush=True)
try:
    while True:
        try: command=input()
        except EOFError: break
        if command=="count":print("COUNT",count,flush=True)
        if command=="quit":break
finally:
    p.terminate()
    try:p.wait(timeout=2)
    except subprocess.TimeoutExpired:p.kill()
'''

parser=argparse.ArgumentParser()
parser.add_argument("--port",default="/dev/ttyUSB0")
parser.add_argument("--host",default="192.168.31.209")
parser.add_argument("--cycles",type=int,default=5)
parser.add_argument("--skip-hci",action="store_true")
parser.add_argument("--skip-reconnect",action="store_true",help="Do not deliberately disconnect/reconnect host 2 after switching tests")
args=parser.parse_args()
port=serial.Serial(port=None,baudrate=115200,timeout=.1)
port.dtr=False;port.rts=False;port.port=args.port;port.open()
uart=[]
monitor=None
poller=selectors.DefaultSelector()
row=re.compile(r"Slot (\d+): (paired|empty) connected=(\d) encrypted=(\d) notify=0x([0-9a-f]+) conn=(-?\d+) tx=(\d+),(\d+),(\d+)")

def read_for(seconds):
    end=time.monotonic()+seconds
    chunks=[]
    while time.monotonic()<end:
        data=port.read(4096).decode(errors="replace")
        chunks.append(data);uart.append(data)
    return "".join(chunks)

def snapshot():
    port.write(b"s\n")
    text=read_for(.25)
    peers={}
    for m in row.finditer(text):
        peers[int(m[1])] = dict(paired=m[2]=="paired",connected=m[3]=="1",
            encrypted=m[4]=="1",notify=int(m[5],16),connection=int(m[6]),
            tx=tuple(map(int,m.group(7,8,9))))
    return peers,text

def assert_online(peers,ids=None):
    for slot in (1,2):
        assert slot in peers,f"missing slot {slot} status"
        p=peers[slot]
        assert p["paired"] and p["connected"] and p["encrypted"] and p["notify"]==7, (slot,p)
        if ids:assert p["connection"]==ids[slot],f"slot {slot} reconnected during switching"

def remote_line():
    if not poller.select(8):raise AssertionError("SSH monitor timeout")
    line=monitor.stdout.readline().strip()
    assert line,line or "SSH monitor closed"
    return line

def remote_count():
    if monitor is None:return None
    monitor.stdin.write("count\n");monitor.stdin.flush()
    line=remote_line()
    assert line.startswith("COUNT "),line
    return int(line.split()[1])

def remote_connection(action):
    assert action in ("Connect","Disconnect")
    code=("import dbus; b=dbus.SystemBus(); "
          "d=dbus.Interface(b.get_object('org.bluez','/org/bluez/hci0/dev_68_EE_8F_59_7F_36',introspect=False),'org.bluez.Device1'); "
          f"d.{action}(timeout=15); print('{action}: target only')")
    subprocess.run(["ssh","-o","BatchMode=yes","-o","ConnectTimeout=8",args.host,
        "python3 -c "+shlex.quote(code)],check=True,timeout=25)

try:
    # Some boards reset when a CH340 serial port is opened; both bonded hosts
    # must independently recover their links, encryption and CCCD state first.
    deadline=time.monotonic()+35
    while True:
        peers,text=snapshot()
        try:assert_online(peers)
        except AssertionError:
            if time.monotonic()>deadline:raise
            read_for(.5)
        else:break
    read_for(2)
    peers,_=snapshot();assert_online(peers)
    ids={s:peers[s]["connection"] for s in (1,2)}
    print("PASS: both bonded hosts online, encrypted and subscribed",ids,flush=True)
    stable_at=len("".join(uart))

    if not args.skip_hci:
        monitor=subprocess.Popen(["ssh","-o","BatchMode=yes","-o","ConnectTimeout=8",args.host,
            "python3 -c "+shlex.quote(MONITOR)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,text=True,bufsize=1)
        poller.register(monitor.stdout,selectors.EVENT_READ)
        assert remote_line()=="READY"

    for cycle in range(args.cycles):
        for target in (1,2):
            port.write(f"{target}\n".encode());read_for(.3)
            before,_=snapshot();assert_online(before,ids)
            hci_before=remote_count()
            port.write(b"t\n");read_for(.4)
            after,_=snapshot();assert_online(after,ids)
            hci_after=remote_count()
            for slot in (1,2):
                delta=tuple(b-a for a,b in zip(before[slot]["tx"],after[slot]["tx"]))
                if slot==target:assert all(n>=1 for n in delta),(target,slot,delta)
                else:assert delta==(0,0,0),f"input leaked to inactive slot {slot}: {delta}"
            if hci_before is not None:
                n=hci_after-hci_before
                if target==2:assert n>=3,f"second host received only {n} HID notifications"
                else:assert n==0,f"second host received {n} notifications while slot 1 was active"
        print(f"PASS: cycle {cycle+1}, routing isolated; connection IDs unchanged",flush=True)

    # Selecting an offline slot must never disconnect or retarget either peer.
    port.write(b"3\n");read_for(.3)
    peers,text=snapshot();assert_online(peers,ids)
    assert "Selected slot=3 connected=0 ready=0" in text,text
    port.write(b"1\n");read_for(.3)
    peers,_=snapshot();assert_online(peers,ids)
    stable="".join(uart)[stable_at:]
    assert "Disconnected conn=" not in stable,"a BLE link disconnected during switching"
    assert "Authentication failed" not in stable,"authentication failed during switching"
    assert "Notification failed" not in stable,"notification delivery failed"
    assert " E (" not in stable,"ESP error observed; inspect UART log"
    print("PASS: offline slot selection preserves both links; no disconnect or notification errors",flush=True)

    if not args.skip_reconnect:
        # Explicitly exercise one host going away. This is not a target switch;
        # the still-online host must retain its connection and subscriptions.
        reconnect_at=len("".join(uart))
        remote_connection("Disconnect");read_for(.5)
        peers,_=snapshot()
        assert peers[1]["connected"] and peers[1]["connection"]==ids[1]
        assert not peers[2]["connected"],peers[2]
        remote_connection("Connect")
        deadline=time.monotonic()+20
        while True:
            peers,_=snapshot()
            assert peers[1]["connected"] and peers[1]["connection"]==ids[1],"host 1 disturbed by host 2 reconnect"
            if peers[2]["connected"] and peers[2]["encrypted"] and peers[2]["notify"]==7:break
            assert time.monotonic()<deadline,"host 2 failed to recover encryption/subscriptions"
            read_for(.3)
        reconnect_log="".join(uart)[reconnect_at:]
        assert reconnect_log.count("Disconnected conn=")==1,"unexpected duplicate/extra disconnect during reconnect"
        assert "Authentication failed" not in reconnect_log
        assert " E (" not in reconnect_log
        print("PASS: host 2 disconnect/reconnect restores encryption and CCCD; host 1 stays online",flush=True)
finally:
    if monitor:
        try:monitor.stdin.write("quit\n");monitor.stdin.flush();monitor.wait(timeout=5)
        except (BrokenPipeError,subprocess.TimeoutExpired):monitor.terminate()
    port.close()
    path=Path("/tmp/opencode/bluetooth-switch-multilink-uart.log")
    path.write_text("".join(uart))
    print("UART log:",path)
