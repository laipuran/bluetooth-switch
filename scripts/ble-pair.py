#!/usr/bin/env python3
"""对指定 BT Switch 配对并验证连接；不删除配对、不接纳其他设备的授权。"""

import argparse
import json

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

AGENT_INTERFACE = "org.bluez.Agent1"
AGENT_PATH = "/bt_switch_test_agent"


class Agent(dbus.service.Object):
    def __init__(self, bus, target):
        super().__init__(bus, AGENT_PATH)
        self.target = target

    def check(self, device):
        if str(device) != self.target:
            raise dbus.exceptions.DBusException("非测试设备", name="org.bluez.Error.Rejected")

    @dbus.service.method(AGENT_INTERFACE, in_signature="", out_signature="")
    def Release(self):
        pass

    @dbus.service.method(AGENT_INTERFACE, in_signature="", out_signature="")
    def Cancel(self):
        pass

    @dbus.service.method(AGENT_INTERFACE, in_signature="ou", out_signature="")
    def RequestConfirmation(self, device, _passkey):
        self.check(device)

    @dbus.service.method(AGENT_INTERFACE, in_signature="o", out_signature="")
    def RequestAuthorization(self, device):
        self.check(device)

    @dbus.service.method(AGENT_INTERFACE, in_signature="os", out_signature="")
    def AuthorizeService(self, device, uuid):
        self.check(device)
        if str(uuid) != "00001812-0000-1000-8000-00805f9b34fb":
            raise dbus.exceptions.DBusException("非 HID 服务", name="org.bluez.Error.Rejected")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    args = parser.parse_args()
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SystemBus()
    target = "/org/bluez/hci0/dev_" + args.address.upper().replace(":", "_")
    agent = Agent(bus, target)
    manager = dbus.Interface(bus.get_object("org.bluez", "/org/bluez"), "org.bluez.AgentManager1")
    adapter = dbus.Interface(bus.get_object("org.bluez", "/org/bluez/hci0"), "org.bluez.Adapter1")
    loop = GLib.MainLoop()
    result = {"paired": False, "error": None}

    def failed(error):
        result["error"] = str(error)
        loop.quit()

    def paired():
        result["paired"] = True
        device = dbus.Interface(bus.get_object("org.bluez", target), "org.bluez.Device1")
        device.Connect(reply_handler=lambda: None, error_handler=failed, timeout=25)

    def pair():
        try:
            device = dbus.Interface(bus.get_object("org.bluez", target), "org.bluez.Device1")
            device.Pair(reply_handler=paired, error_handler=failed, timeout=40)
        except dbus.exceptions.DBusException as error:
            failed(error)
        return False

    def check_ready():
        if result["paired"]:
            props = dbus.Interface(bus.get_object("org.bluez", target), "org.freedesktop.DBus.Properties")
            values = props.GetAll("org.bluez.Device1")
            if values.get("Connected") and values.get("ServicesResolved"):
                loop.quit()
                return False
        return True

    manager.RegisterAgent(AGENT_PATH, "NoInputNoOutput")
    # 不注册成系统默认代理，避免改变其他蓝牙设备的配对行为。
    adapter.SetDiscoveryFilter({"Transport": dbus.String("le")})
    discovery_started = False
    try:
        adapter.StartDiscovery()
        discovery_started = True
        GLib.timeout_add_seconds(5, pair)
        GLib.timeout_add_seconds(1, check_ready)
        GLib.timeout_add_seconds(55, lambda: (failed("配对/服务解析超时"), False)[1])
        loop.run()

        if result["error"]:
            raise SystemExit("FAIL: " + result["error"])

        props = dbus.Interface(bus.get_object("org.bluez", target), "org.freedesktop.DBus.Properties")
        values = props.GetAll("org.bluez.Device1")
        snapshot = {key: bool(values.get(key)) for key in ("Paired", "Connected", "ServicesResolved")}
        snapshot["HidService"] = "00001812-0000-1000-8000-00805f9b34fb" in values.get("UUIDs", [])
        print(json.dumps(snapshot))
        if not all(snapshot.values()):
            raise SystemExit("FAIL: 配对、连接或 HID 服务解析未通过")
        print("PASS: 新配对、连接和 HID 服务解析成功；不代表 USB 输入已验证")
    finally:
        if discovery_started:
            adapter.StopDiscovery()
        manager.UnregisterAgent(AGENT_PATH)


if __name__ == "__main__":
    main()
