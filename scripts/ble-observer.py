#!/usr/bin/env python3
"""JSON 行协议的 HID 观测端：仅订阅、读写连接属性，不合成键鼠输入。"""

import argparse
import json
import sys
import time

import dbus
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


class Observer:
    def __init__(self, bus, target):
        self.bus = bus
        self.target = target
        self.inputs = {}
        self.mutable = {}
        self.subscribed = []
        self.notifications = []
        objects = dbus.Interface(bus.get_object("org.bluez", "/"), "org.freedesktop.DBus.ObjectManager").GetManagedObjects()
        for path, interfaces in objects.items():
            if not str(path).startswith(target + "/"):
                continue
            descriptor = interfaces.get("org.bluez.GattDescriptor1")
            if descriptor and str(descriptor.get("UUID")) == self.uuid(0x2908):
                reference = bytes(self.interface(path, "org.bluez.GattDescriptor1").ReadValue({}))
                if len(reference) == 2 and reference[1] == 1:
                    self.inputs[str(descriptor["Characteristic"])] = f"report{reference[0]}"
            characteristic = interfaces.get("org.bluez.GattCharacteristic1")
            if characteristic:
                uuid = str(characteristic["UUID"])
                for short, name in [(0x2a4e, "protocol"), (0x2a4c, "control"), (0x2a22, "boot_keyboard"), (0x2a33, "boot_mouse")]:
                    if uuid == self.uuid(short):
                        self.mutable[name] = str(path)
                if uuid == self.uuid(0x2a4d) and "write" in characteristic.get("Flags", []):
                    self.mutable["leds"] = str(path)
        if set(self.inputs.values()) != {"report1", "report2", "report3"}:
            raise RuntimeError("缺少三种 HID 输入报告")
        bus.add_signal_receiver(self.changed, signal_name="PropertiesChanged", dbus_interface="org.freedesktop.DBus.Properties", path_keyword="path")

    @staticmethod
    def uuid(short):
        return f"0000{short:04x}-0000-1000-8000-00805f9b34fb"

    def interface(self, path, name="org.bluez.GattCharacteristic1"):
        return dbus.Interface(self.bus.get_object("org.bluez", path), name)

    def subscribe(self, boot=False):
        paths = dict(self.inputs)
        if boot:
            paths.update({self.mutable[name]: name for name in ("boot_keyboard", "boot_mouse")})
        for path, name in paths.items():
            self.inputs[path] = name
            if path not in self.subscribed:
                self.interface(path).StartNotify()
                self.subscribed.append(path)

    def changed(self, interface, values, _invalidated, path=None):
        if interface == "org.bluez.GattCharacteristic1" and str(path) in self.inputs and "Value" in values:
            data = bytes(values["Value"])
            self.notifications.append({"report": self.inputs[str(path)], "length": len(data), "neutral": not any(data), "time": time.monotonic()})

    def snapshot(self):
        properties = self.interface(self.target, "org.freedesktop.DBus.Properties").GetAll("org.bluez.Device1")
        protocol = bytes(self.interface(self.mutable["protocol"]).ReadValue({}))[0]
        leds = bytes(self.interface(self.mutable["leds"]).ReadValue({}))[0]
        return {"connected": bool(properties["Connected"]), "resolved": bool(properties["ServicesResolved"]), "protocol": protocol, "leds": leds, "notifications": list(self.notifications)}

    def command(self, request):
        action = request["action"]
        if action == "reset":
            self.notifications.clear()
            return {"reset": True}
        if action == "subscribe":
            self.subscribe(request.get("boot", False))
            return {"subscribed": True}
        if action in ("protocol", "control", "leds"):
            value = int(request["value"])
            if not 0 <= value <= (255 if action == "leds" else 1):
                raise ValueError("属性值越界")
            options = {"type": dbus.String("request" if action == "leds" else "command")}
            self.interface(self.mutable[action]).WriteValue(dbus.Array([value], signature="y"), options)
            return {"written": action, "value": value}
        if action == "status":
            return self.snapshot()
        raise ValueError("未知命令")

    def close(self):
        for path in reversed(self.subscribed):
            try:
                self.interface(path).StopNotify()
            except dbus.exceptions.DBusException as error:
                print(f"StopNotify: {error}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    args = parser.parse_args()
    DBusGMainLoop(set_as_default=True)
    target = "/org/bluez/hci0/dev_" + args.address.upper().replace(":", "_")
    observer = Observer(dbus.SystemBus(), target)
    loop = GLib.MainLoop()

    def reply(data):
        print(json.dumps(data), flush=True)

    def receive(_source, _condition):
        line = sys.stdin.readline()
        if not line:
            loop.quit()
            return False
        try:
            reply(observer.command(json.loads(line)))
        except Exception as error:
            reply({"error": str(error)})
        return True

    GLib.io_add_watch(sys.stdin, GLib.IO_IN | GLib.IO_HUP, receive)
    reply({"ready": True})
    try:
        loop.run()
    finally:
        observer.close()


if __name__ == "__main__":
    main()
