#!/usr/bin/env python3
"""只记录目标设备的 HCI/SMP 安全元数据；不输出密钥、HID 报告或原始包。"""

import argparse
import json
import select
import socket
import struct
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--seconds", type=float, default=45)
    args = parser.parse_args()
    target = bytes.fromhex(args.address.replace(":", ""))[::-1]
    monitor = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW, socket.BTPROTO_HCI)
    monitor.bind((0xffff, 2))
    handles = set()
    commands = {}

    def emit(event, **fields):
        print(json.dumps({"time": time.monotonic(), "event": event, **fields}), flush=True)

    emit("monitor_ready")
    deadline = time.monotonic() + args.seconds
    try:
        while time.monotonic() < deadline:
            if not select.select([monitor], [], [], max(0, deadline - time.monotonic()))[0]:
                break
            frame = monitor.recv(65535)
            if len(frame) < 6:
                continue
            opcode, index, _length = struct.unpack_from("<HHH", frame)
            packet = frame[6:]
            if opcode == 3 and len(packet) >= 2:
                event = packet[0]
                data = packet[2:]
                if event == 0x3e and len(data) >= 12 and data[0] in (1, 10):
                    kind, body = data[0], data[1:]
                    if body[5:11] == target and body[0] == 0:
                        handle = struct.unpack_from("<H", body, 1)[0]
                        handles.add((index, handle))
                        interval_offset = 23 if kind == 10 else 11
                        interval = struct.unpack_from("<H", body, interval_offset)[0] if len(body) >= interval_offset + 2 else 0
                        emit("connected", index=index, handle=handle, interval_ms=interval * 1.25)
                elif event in (8, 0x30, 0x59) and len(data) >= 3:
                    handle = struct.unpack_from("<H", data, 1)[0]
                    if (index, handle) in handles:
                        emit("encryption_change" if event != 0x30 else "key_refresh", handle=handle, status=data[0], encrypted=data[3] if len(data) > 3 else None)
                elif event == 5 and len(data) >= 4:
                    handle = struct.unpack_from("<H", data, 1)[0]
                    if (index, handle) in handles:
                        emit("disconnected", handle=handle, reason=data[3])
                        handles.discard((index, handle))
                elif event == 15 and len(data) >= 4:
                    command = struct.unpack_from("<H", data, 2)[0]
                    if (index, command) in commands:
                        emit("command_status", command=command, handle=commands[(index, command)], status=data[0])
            elif opcode == 2 and len(packet) >= 5:
                command = struct.unpack_from("<H", packet)[0]
                if command in (0x2019, 0x201a, 0x201b):
                    handle = struct.unpack_from("<H", packet, 3)[0]
                    if (index, handle) in handles:
                        commands[(index, command)] = handle
                        emit("security_command", command=command, handle=handle)
                    else:
                        commands.pop((index, command), None)
            elif opcode in (4, 5) and len(packet) >= 9:
                flags = struct.unpack_from("<H", packet)[0]
                handle = flags & 0xfff
                if (index, handle) not in handles or (flags >> 12) & 3 == 1:
                    continue
                cid = struct.unpack_from("<H", packet, 6)[0]
                if cid == 6:
                    smp = packet[8:]
                    fields = {"handle": handle, "direction": "tx" if opcode == 4 else "rx", "opcode": smp[0]}
                    if smp[0] in (5, 11) and len(smp) > 1:
                        fields["reason_or_authreq"] = smp[1]
                    emit("smp", **fields)
    finally:
        monitor.close()


if __name__ == "__main__":
    main()
