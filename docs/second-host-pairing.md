# 第二台 Linux 电脑配对失败的诊断

本文记录旧版单连接固件期间的配对代理诊断。后续多连接改造及验证见 [multilink.md](multilink.md)；电脑端缺少配对代理的问题与连接数量无关。

## 结论

第二台电脑（`192.168.31.209`）的 BlueZ **没有可用的配对代理（Agent1）**，因此拒绝配对授权。不是 USB VBUS 供电问题，也没有证据表明 ESP32 漏处理了数字比较事件。

第二台电脑的系统日志给出直接证据：

```text
src/device.c:new_auth() No agent available for request type 2
device_confirm_passkey: Operation not permitted
```

BlueZ `src/device.c` 中 request type 2 是 `AUTH_TYPE_CONFIRM`。这里的“确认”也覆盖 Just Works 授权，不代表用户必须看见或输入六位数字。

## 对照复现

保持 ESP32 固件不变，电脑 1 的蓝牙保持用户原先关闭的状态，选择空槽位 2：

| 第二台电脑的配对路径 | 结果 |
| --- | --- |
| 使用系统现有默认代理，不额外注册代理 | BlueZ 报没有可用代理；配对失败，ESP32 重现 `Authentication failed: 0x59` |
| 临时注册只针对 BT Switch 的 `NoInputNoOutput` 代理 | 配对成功；ESP32 保存槽位 2，达到 `ready=1` |

协议抓取确认双方 IO 能力都是 `NoInputNoOutput (0x03)`，使用 Secure Connections Just Works。失败路径返回 SMP `Numeric comparison failed (0x0c)`，ESP32 将该失败原因映射为 `0x59`。**仅依据错误名称推断“漏处理数字比较请求”是不成立的。**

成功路径最终确认：

```text
Paired=True
Bonded=True
Connected=True
ServicesResolved=True
Selected slot=2 connected=1 ready=1 pairing=0 bonds=2
```

诊断结束时保留槽位 1 原配对，以及槽位 2 新配对；临时代理、发现会话和 btmon 均已退出。没有修改/重新烧录固件，没有设为全局自动接受配对，也没有修改 Trusted 属性（第二台电脑上为 False）。

## 后续处理

- 新设备配对前，确保蓝牙管理器注册了配对代理。可重新启动使用的图形蓝牙管理器；仅重启 bluetooth 服务可能反而丢失此前的代理注册。
- 后续对比已确认 DMS 的代理恢复缺陷，见下节。临时处理是在 bluetooth 服务启动后重新启动 DMS，而不是继续反复重启 bluetooth。
- 命令行手动配对可以保持一个交互式 `bluetoothctl` 会话，执行 `agent NoInputNoOutput`、`default-agent`，再执行目标设备的 `pair`。该会话退出时代理也会注销。
- 不要以此为由清空所有 NVS、删除电脑 1 的配对或在固件中无条件自动确认数字比较。
- 电脑 1 抢连空槽位的架构问题、重复 Disconnected 回调仍是独立问题，本次没有修复或宣称解决。

## 为什么两台电脑同用 DMS，只有第二台失败

两台电脑都使用 DMS v2.1.2，构建元数据中的源码 revision 均为 `43255a46ea01e9d0de1fe8ce9897c0d71566b2a8`。区别在于服务生命周期：

- 当前电脑：bluetoothd 先启动，DMS 后启动；没有发生 bluetoothd 晚于 DMS 启动的情况。
- 第二台电脑：2026-11-09 08:57:43，DMS 日志记录 `BluezAgent registered ... KeyboardDisplay`，说明最初正常注册。
- 当天 10:22:18，bluetooth 服务重启，DMS 日志记录 `BluezAgent Release called`。
- 此后 bluetooth 又重启过两次，DMS 一直未重启；日志只显示适配器列表更新，没有再次注册代理。BlueZ 持续报告没有可用代理。

对应源码中：

- [`manager.go`](https://github.com/duxlabs-dev/dux-shell/blob/43255a46ea01e9d0de1fe8ce9897c0d71566b2a8/core/internal/server/bluez/manager.go#L363) 的 `handleBluezOwnerChanged()` 在 BlueZ 恢复时只调用 `enqueueAdapterRefresh()`，没有恢复配对代理注册。
- [`agent.go`](https://github.com/duxlabs-dev/dux-shell/blob/43255a46ea01e9d0de1fe8ce9897c0d71566b2a8/core/internal/server/bluez/agent.go#L119) 的 `Release()` 仅记录日志，没有重新注册逻辑。

因此，DMS 的面板、设备列表仍然可用，但新的 BlueZ 进程里已经没有原来的 Agent 注册记录。**代理进程仍在运行，与代理仍在 BlueZ 注册，是两件不同的事。**

第二台电脑通过 systemd 用户服务运行 DMS，可执行：

```sh
systemctl --user restart dms.service
```

该操作会短暂重启桌面面板，不删除配对。此次只读排查没有执行它。长期修复应由 DMS 在 BlueZ 的 D-Bus owner 恢复后重新注册 Agent 并恢复默认代理角色。

## 回归工具

```sh
# 默认路径：当前未注册常驻代理时，能够重现原错误。
~/.platformio/penv/bin/python test/reproduce_pairing.py --agent-mode default

# 临时的、仅针对目标设备的无 IO 代理：应能完成首次配对。
~/.platformio/penv/bin/python test/reproduce_pairing.py --agent-mode noinput
```

前提：串口空闲，槽位 2 为空，电脑 2 尚未保存 BT Switch 的配对，电脑 1 不抢占开放配对窗口。工具会选择槽位 2、打开配对窗口并尝试配对；**不会自动删除任何配对**，发现已有配对会跳过并返回非成功状态。因此现在已有成功配对时，不应为了运行测试随意删除记录。

工具只保留 SMP 请求/响应元数据及错误，不输出密钥、配对码。需要本机 PlatformIO Python 的 pyserial，第二台电脑的 Python dbus/PyGObject，以及第二台电脑上用于 btmon 的免交互 sudo 权限。
