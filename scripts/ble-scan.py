#!/usr/bin/env python3
"""通过 BlueZ 观察目标的实际 BLE 广播，不配对、不连接、不删除已有主机信息。"""

import argparse
import json

import dbus
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--adapter", default="/org/bluez/hci0")
    parser.add_argument("--seconds", type=int, default=15)
    args = parser.parse_args()

    DBusGMainLoop(set_as_default=True)
    bus = dbus.SystemBus()
    manager = dbus.Interface(bus.get_object("org.bluez", "/"), "org.freedesktop.DBus.ObjectManager")
    adapter = dbus.Interface(bus.get_object("org.bluez", args.adapter), "org.bluez.Adapter1")
    target = args.adapter + "/dev_" + args.address.upper().replace(":", "_")
    seen = []
    loop = GLib.MainLoop()

    def record(path, values):
        # 缓存的设备对象不算成功，必须收到本次扫描中的无线观测更新。
        if str(path) != target or not any(key in values for key in ("RSSI", "ManufacturerData", "ServiceData")):
            return

        properties = dbus.Interface(bus.get_object("org.bluez", path), "org.freedesktop.DBus.Properties")
        info = properties.GetAll("org.bluez.Device1")
        seen.append({
            "name": str(info.get("Name", "")),
            "address": str(info.get("Address", "")),
            "rssi": int(info.get("RSSI", 0)),
            "uuids": [str(uuid) for uuid in info.get("UUIDs", [])],
        })

    def changed(interface, values, _invalidated, path=None):
        if interface == "org.bluez.Device1":
            record(path, values)

    def added(path, interfaces):
        if "org.bluez.Device1" in interfaces:
            record(path, interfaces["org.bluez.Device1"])

    bus.add_signal_receiver(changed, signal_name="PropertiesChanged", dbus_interface="org.freedesktop.DBus.Properties", path_keyword="path")
    manager.connect_to_signal("InterfacesAdded", added)

    adapter.SetDiscoveryFilter({"Transport": dbus.String("le"), "DuplicateData": dbus.Boolean(True)})
    adapter.StartDiscovery()
    try:
        GLib.timeout_add_seconds(args.seconds, lambda: (loop.quit(), False)[1])
        loop.run()
    finally:
        # 仅结束本客户端的发现会话，不停止其他客户端的会话。
        adapter.StopDiscovery()

    if not seen:
        raise SystemExit("FAIL: 本次扫描未收到目标的广播观测更新；缓存对象不计入成功")

    latest = seen[-1]
    print(json.dumps(latest, ensure_ascii=False))
    if latest["name"] != "BT Switch" or "00001812-0000-1000-8000-00805f9b34fb" not in latest["uuids"]:
        raise SystemExit("FAIL: 广播设备名称或 HID 服务 UUID 不符合预期")
    print("PASS: 实际观测到 BT Switch BLE HID 广播（不代表已配对或输入链路通过）")


if __name__ == "__main__":
    main()
