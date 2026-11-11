#!/usr/bin/env python3
"""检查本机与 SSH 主机是否同时连接 BT Switch；不清理配对、不切输入目标。"""

import argparse
import json
import shlex
import subprocess
import time
from pathlib import Path

import dbus


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remote", required=True)
    parser.add_argument("--ssh-command", default="ssh -o BatchMode=yes -o ConnectTimeout=8", help="例如 'tailscale ssh'；不经 shell 展开")
    parser.add_argument("--address", required=True)
    parser.add_argument("--power-on-remote", action="store_true")
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    target = "/org/bluez/hci0/dev_" + args.address.upper().replace(":", "_")
    bus = dbus.SystemBus()

    def local_snapshot():
        props = dbus.Interface(bus.get_object("org.bluez", target), "org.freedesktop.DBus.Properties")
        values = props.GetAll("org.bluez.Device1")
        return {key: bool(values.get(key)) for key in ("Paired", "Connected", "ServicesResolved")}

    def remote(code):
        result = subprocess.run(
            [*shlex.split(args.ssh_command), args.remote, "/usr/bin/python3", "-"],
            input=("import dbus, json\nbus=dbus.SystemBus()\n" + f"target={target!r}\n" + code),
            text=True, capture_output=True, timeout=30, check=True,
        )
        return json.loads(result.stdout)

    snapshot_code = """
adapter=dbus.Interface(bus.get_object('org.bluez','/org/bluez/hci0'),'org.freedesktop.DBus.Properties')
props=dbus.Interface(bus.get_object('org.bluez',target),'org.freedesktop.DBus.Properties')
values=props.GetAll('org.bluez.Device1')
state={key:bool(values.get(key)) for key in ('Paired','Connected','ServicesResolved')}
state['Powered']=bool(adapter.Get('org.bluez.Adapter1','Powered'))
print(json.dumps(state))
"""
    initial_remote = remote(snapshot_code)
    result = {"initial_local": local_snapshot(), "initial_remote": initial_remote}
    passed = False

    try:
        if not result["initial_local"]["Connected"]:
            raise RuntimeError("第一台主机未连接，无法检验第二条并发连接")
        if not initial_remote["Powered"]:
            if not args.power_on_remote:
                raise RuntimeError("第二台蓝牙已关闭；需显式允许临时开启")
            remote("""
adapter=dbus.Interface(bus.get_object('org.bluez','/org/bluez/hci0'),'org.freedesktop.DBus.Properties')
adapter.Set('org.bluez.Adapter1','Powered',dbus.Boolean(True))
print(json.dumps({'powered_on':True}))
""")

        result["connect_attempt"] = remote("""
device=dbus.Interface(bus.get_object('org.bluez',target),'org.bluez.Device1')
try:
 device.Connect(timeout=12)
 print(json.dumps({'accepted':True}))
except dbus.exceptions.DBusException as error:
 print(json.dumps({'accepted':False,'error':str(error)}))
""")

        observations = []
        for _ in range(3):
            state = {"remote": remote(snapshot_code), "local": local_snapshot()}
            observations.append(state)
            if not state["local"]["Connected"] or not state["remote"]["Connected"]:
                break
            time.sleep(1)
        result["observations"] = observations
        passed = len(observations) == 3 and all(
            state[host]["Connected"] for state in observations for host in ("local", "remote")
        )
        result["simultaneously_connected"] = passed
    except Exception as error:
        result["test_error"] = str(error)
    finally:
        # 失败时也取消本次连接请求，并恢复远端原来的蓝牙开关状态。
        try:
            if not initial_remote["Connected"]:
                remote("""
device=dbus.Interface(bus.get_object('org.bluez',target),'org.bluez.Device1')
try:
 device.Disconnect(timeout=5)
 print(json.dumps({'disconnected':True}))
except dbus.exceptions.DBusException as error:
 print(json.dumps({'disconnect_error':str(error)}))
""")
            if not initial_remote["Powered"] and args.power_on_remote:
                remote("""
adapter=dbus.Interface(bus.get_object('org.bluez','/org/bluez/hci0'),'org.freedesktop.DBus.Properties')
adapter.Set('org.bluez.Adapter1','Powered',dbus.Boolean(False))
print(json.dumps({'powered_off':True}))
""")
            result["final_remote"] = remote(snapshot_code)
            result["final_local"] = local_snapshot()
        except Exception as error:
            result["cleanup_error"] = str(error)
            passed = False

        Path(args.log).write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")

    print(json.dumps(result, indent=2, ensure_ascii=False))
    if not passed:
        raise SystemExit("FAIL: 未验证两台主机同时保持连接；不删除旧配对来强行消除失败")
    print("PASS: 两台主机同时保持连接；尚未验证逐主机通知隔离或切换延迟")


if __name__ == "__main__":
    main()
