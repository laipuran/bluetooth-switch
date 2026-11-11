#!/usr/bin/env python3
"""订阅三个 HID 输入报告，再通过串口 t 验证实际全零通知，不合成输入。"""

import argparse
import os
import termios

import dbus
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--port", default="/dev/ttyUSB0")
    args = parser.parse_args()
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SystemBus()
    manager = dbus.Interface(bus.get_object("org.bluez", "/"), "org.freedesktop.DBus.ObjectManager")
    prefix = "/org/bluez/hci0/dev_" + args.address.upper().replace(":", "_") + "/"
    objects = manager.GetManagedObjects()
    reports = {}

    for path, interfaces in objects.items():
        descriptor = interfaces.get("org.bluez.GattDescriptor1")
        if not str(path).startswith(prefix) or not descriptor:
            continue
        if str(descriptor.get("UUID")) != "00002908-0000-1000-8000-00805f9b34fb":
            continue

        remote = dbus.Interface(bus.get_object("org.bluez", path), "org.bluez.GattDescriptor1")
        reference = bytes(remote.ReadValue({}))
        if len(reference) == 2 and reference[1] == 1 and reference[0] in (1, 2, 3):
            reports[str(descriptor["Characteristic"])] = reference[0]

    if set(reports.values()) != {1, 2, 3}:
        raise SystemExit("FAIL: 未找到完整的键盘、鼠标、多媒体输入报告")

    sizes = {1: 8, 2: 7, 3: 2}
    received = {}
    errors = []
    loop = GLib.MainLoop()

    def changed(interface, values, _invalidated, path=None):
        if interface != "org.bluez.GattCharacteristic1" or str(path) not in reports or "Value" not in values:
            return

        report = reports[str(path)]
        data = bytes(values["Value"])
        if data != bytes(sizes[report]):
            errors.append(f"报告 {report} 非全零或长度错误")
            loop.quit()
            return

        received[report] = len(data)
        if len(received) == 3:
            loop.quit()

    bus.add_signal_receiver(changed, signal_name="PropertiesChanged", dbus_interface="org.freedesktop.DBus.Properties", path_keyword="path")
    subscribed = []
    fd = None
    original = None
    try:
        for path in reports:
            characteristic = dbus.Interface(bus.get_object("org.bluez", path), "org.bluez.GattCharacteristic1")
            characteristic.StartNotify()
            subscribed.append(characteristic)

        fd = os.open(args.port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        original = termios.tcgetattr(fd)
        config = termios.tcgetattr(fd)
        config[0] = 0
        config[1] = 0
        config[2] = termios.CS8 | termios.CLOCAL | termios.CREAD
        config[3] = 0
        config[4] = termios.B115200
        config[5] = termios.B115200
        termios.tcsetattr(fd, termios.TCSANOW, config)

        # 先建立所有订阅，再请求显式探针，避免依赖初次解锁的发送时机。
        os.write(fd, b"t")
        GLib.timeout_add_seconds(8, lambda: (loop.quit(), False)[1])
        loop.run()
    finally:
        if fd is not None:
            if original is not None:
                termios.tcsetattr(fd, termios.TCSANOW, original)
            os.close(fd)
        for characteristic in reversed(subscribed):
            characteristic.StopNotify()

    if errors or set(received) != {1, 2, 3}:
        raise SystemExit(f"FAIL: received={received}, errors={errors}")

    print(f"PASS: 实际收到三份全零 HID 通知，报告长度 {received}")
    print("仅验证 BLE 订阅与通知，不代表 USB 输入或多连接通过。")


if __name__ == "__main__":
    main()
