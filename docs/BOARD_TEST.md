# Rust 固件上板测试记录

本文件保留不同阶段的测试证据，最新结果为 2026-11-13 的 USB 输入与鼠标频率对照；后文单机及旧版本记录属于历史，不代表当前实现状态。

## 最新：2026-11-13 USB、单键与鼠标移动观测

- 原生 USB 的焊桥由用户焊接，已确认能向外供电；软件的 `power_command` 不是 VBUS 电压测量。
- 启用 Hub 和多级 Hub。可用 Hub `05e3:0610` 下识别到 `3554:fa09` 接收器及 `35bb:d011 HECATE Wireless Receiver`，主固件共 4 个 HID 接口，可解码真实输入。
- 此前多功能扩展坞在电脑端可枚举 USB2 Hub 和接收器，但板端一直 `connected=false/line_state=0`；空载也未改善。不能据此确诊 CC、供电或芯片兼容性。
- 外接按键 S=GPIO4、负极=GND、正极=3V3，显式启用 `prototype-button`；15 ms 消抖、短按选择下一个已配对槽位，长按 3 秒打开空槽配对窗口。配对但离线的槽位不算空槽。
- 测速脚本 `scripts/mouse-timing-check.py` 只读取指定 evdev 鼠标的非零 XY 帧时间，不独占设备、不保存轨迹或按键。每轮持续移动约 25 秒，包含停顿；不是 USB→BLE 端到端延迟或显示刷新测量。

| 观测场景 | 平均移动帧 Hz | P95 ms | P99 ms | 最大 ms |
| --- | ---: | ---: | ---: | ---: |
| G2M Pro 接收器直插本电脑 | 996.43 | 1.004 | 1.038 | 5.001 |
| 桥接、确认 BLE 30 ms、逐报告日志开启 | 97.64 | 59.990 | 60.091 | 90.972 |
| 桥接、确认 BLE 15 ms、逐报告日志开启 | 100.80 | 15.107 | 16.821 | 240.092 |
| 恢复按钮版、未核实 BLE 实际间隔 | 94.33 | 22.001 | 23.007 | 24.072 |
| 关闭逐报告日志、测试前后确认本电脑 7.5 ms、另一台 30 ms | 114.57 | 15.984 | 23.002 | 248.062 |

最后一轮两台均加密在线、`subs=0x07`，未收到逐报告 INFO 日志。日志开关和实际连接间隔同时变化，
不能把性能差异全部归因于关闭日志。脚本 P95 ≤10 ms 是人为诊断目标，不是产品规格。

7.5 ms 的运行时更新曾在两台在线时被拒绝；临时断开另一台后实际达到 7.5 ms。
该次随后出现零移动帧且连接条件变化，结果为 INCONCLUSIVE，不用于性能结论。
恢复双机并重刷后又实测到两台在线、其中一台 7.5 ms，故不能声称双连接有 15 ms 硬上限。
临时阻止另一台自动回连的设置已恢复，配对未删除。最终固件只查询实际连接参数，不自动发起更新。

测试原始结果保存在本机 `/tmp/opencode/`，不随仓库分发：
`g2m-pro-usb-direct-continuous.json`、`bt-switch-mouse-baseline-continuous.json`、
`bt-switch-mouse-15ms-continuous.json`、`bt-switch-restored-button-mouse-timing.json`、
`bt-switch-no-report-log-75ms.json`，对应串口状态日志为 `bt-switch-no-report-log-before/after.serial.log`。

## 最新：2026-11-12 双机实测与修复

### 清理与环境

- 按用户明确授权，对同一块 16 MB ESP32-S3 执行 **整片 `erase-flash`**，不做新备份；随后仅刷入本项目 Rust bootloader、分区表及应用，旧固件和 NVS 已清除。
- 仅清理两台 Linux 的 `BT Switch`（`68:EE:8F:59:7F:36`）配对记录，未删除其他蓝牙设备。
- 两台主机重新配对到槽位 1/2；机器 2 使用 Tailscale IP `100.115.182.59`，SSH 校验此前保存的 `192.168.31.209` 主机公钥，无关闭公钥校验操作。
- 两端都创建了 `BT Switch Keyboard` 和 `BT Switch Mouse` 输入节点。没有生成实际按键或鼠标位移；USB 状态仍是 0 接口、0 报告，所以这不代表 USB→BLE 完整输入测试通过。

### 发现并修复的问题

1. **忘记单槽位一直等待。** 原流程在广告活跃时调用 `BLEDevice::delete_bond`，ESP-IDF v6.1 的 `ble_gap_unpair` 明确返回 `EBUSY`。实际回归 8 秒内未完成；修复后先暂停广播、只删除指定主机，再恢复广告。实测约 0.8 秒内检测到完成，另一台始终保持加密连接。
2. **双机同时恢复加密偶发等待约 30 秒。** 两端 HCI 元数据显示已发送加密命令，一条连接迟迟没有加密完成事件。根因是 NimBLE 的 SMP 过程池仍默认为 1，而连接容量已为 3。应用层串行请求不能阻止两台中心设备同时发起加密。

最终修复通过本地 IDF 构建配置为 `bt` 目标定义 `MYNEWT_VAL_BLE_SM_MAX_PROCS=CONFIG_BT_NIMBLE_MAX_CONNECTIONS`。
实际 `ble_sm.c` 编译命令已确认含 `-DMYNEWT_VAL_BLE_SM_MAX_PROCS=3`。
过程中试过调整主动请求与本地依赖补丁，但单独使用它们仍能复现；**最终均撤掉**，继续使用原版 crates.io `esp32-nimble 0.13.0` 和原有请求调度，仅保留容量修正。

### 最终固件实际结果

| 检查 | 结果 |
| --- | --- |
| 双机连接、服务与加密 | 两端 `Paired/Connected/ServicesResolved=true`，固件 `online=2`、两个槽位均加密 |
| 真实 CCCD | 两条连接均观察到 `subs=0x07` |
| 40 次 1↔2 切换 | 两端始终在线；每次旧、新目标都收到三份全零释放 |
| 41 次显式定向探针 | 选中端收到键盘/鼠标/Consumer，未选中端 0 条，未发现广播串流 |
| 选择空槽位 3 | 保留两条加密连接，`ready=false`，两端均 0 条探针；回到 1 后恢复 |
| 忘记槽位 2 | 完成删除且槽位 1 加密在线；随后机器 2 可重新配对回 2 |
| 连续 10 次硬件复位 | 无重新配对，10/10 在 10 秒调试门槛内恢复两条加密授权连接；脚本检测值约 1.0–1.5 秒 |
| 底层容量 | 实际编译为 connections=3、SMP=3、activities=4、CCCDs=48 |
| 未验证 | 第三台同时在线、RPA、其他系统、USB 实际输入、真实拥塞下的硬件释放重试 |

复位恢复计时从脚本发起连接请求算起，包含 0.5 秒状态采样粒度，不是精确 SMP 时间或断电恢复测量。
订阅观测端退出时会取消其订阅，结束后以实际 CCCD 为准；最终复位/回连用于恢复正常主机连接状态。

最终 40 次切换的主机侧释放观测（毫秒）：

| 观测 | 主机 1 最小/中位/最大 | 主机 2 最小/中位/最大 |
| --- | --- | --- |
| 首份释放 | 38.66 / 95.68 / 131.94 | 66.25 / 182.95 / 243.17 |
| 三份释放均已收到 | 67.25 / 109.09 / 157.89 | 77.31 / 203.61 / 244.15 |

计时包含串口命令、BLE 调度及 BlueZ/观测进程开销。远端单调时钟按最小往返校准，半 RTT 约 1.24 ms；
该数值不覆盖后续时钟漂移和调度误差，不能视为硬实时保证，也不是 USB 输入端到端延迟。

### 回归命令与证据

```sh
python3 scripts/check-ble-capacity.py --build-dir target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-0db32ea7f054f2db/out/build

/usr/bin/python3 scripts/ble-two-host-check.py \
  --remote 100.115.182.59 \
  --ssh-command 'ssh -o BatchMode=yes -o HostKeyAlias=192.168.31.209 -o ConnectTimeout=8' \
  --address 68:EE:8F:59:7F:36 --cycles 20 \
  --log /tmp/opencode/bt-switch-two-host-routing-final.json

/usr/bin/python3 scripts/ble-reboot-check.py \
  --remote 100.115.182.59 \
  --ssh-command 'ssh -o BatchMode=yes -o HostKeyAlias=192.168.31.209 -o ConnectTimeout=8' \
  --address 68:EE:8F:59:7F:36 --max-recovery-seconds 10 \
  --log /tmp/opencode/bt-switch-two-host-reboot.json
```

`ble-forget-check.py` 会真正删除槽位 2，运行前须确认允许清理其配对；结束后需在机器 2 重新配对。
`hci-security-watch.py` 需要 root/CAP_NET_RAW，但仅输出目标设备的安全控制元数据，不输出密钥、原始包或 HID 报告。
测试脚本和 JSON/串口日志是回归证据，不是 Flash 备份。

关键证据（位于临时目录，未提交）：

- `/tmp/opencode/bt-switch-two-host-routing-final.json` 及 `-summary.json`
- `/tmp/opencode/bt-switch-forget-red.json`、`bt-switch-forget-final.json`
- `/tmp/opencode/bt-switch-two-host-reboot-red.json`、`bt-switch-two-host-reboot-final-summary.json`
- `/tmp/opencode/bt-switch-hci-host-1.jsonl`、`bt-switch-hci-host-2.jsonl`

## 2026-11-11：多连接实现接入后的单机回归（历史）

固件现已改为 `Vec<Connection> + Router`，底层配置为 3 条连接、4 个控制器活动、48 条 CCCD 存储。
旧版的单连接状态机、800 ms 计时就绪、切槽主动断连及向全部订阅者广播的发送方式均已替换。

新增实现：

- 已配对主机按身份认领原槽位；新主机使用建连时捕获的配对槽位与窗口。
- 未满连接时继续广播，白名单包含全部已配对槽位。
- 真实 CCCD、协议、挂起、LED 及输入报告读取快照按连接隔离。
- 输入仅通过 `notify_with(data, connection)` 发往目标，失败保留释放欠账重试。
- 切槽仅改变目标并释放旧、新目标；忘记只处理指定槽位。
- 动态 HID 属性和输入 CCCD 写要求加密；另实现 Boot 键盘/鼠标属性及格式转换。

已实际完成的最新回归：

| 检查 | 结果 |
| --- | --- |
| 核心测试 | 33 项通过，含多连接路由、部分释放失败重试、离线目标、逐会话状态及 Boot 转换 |
| 固件构建与 Clippy | 默认通用板和官方 OTG 配置的 Clippy 通过，默认配置 release 构建通过 |
| 升级保留配对 | 未擦除 NVS，原 Rust 槽位 1 主机可加密回连 |
| 连接期间广播 | 串口 `online=1` 且 `advertising=true`，扫描实际收到设备广播 |
| 真实订阅就绪 | 串口观察到 `encrypted=true subs=0x07 releases=0x00`；观测脚本结束、订阅取消后可变为 `subs=0` 且 `ready=false`，不再以计时代替订阅 |
| 定向通知 | 第一台主机收到三种全零 HID 报告，长度 8/7/2 字节 |
| 离线目标隔离 | 选空槽位 2/3 时 `online=1 ready=false`，原主机仍连接，显式 `t` 不向它回退发送 |
| 切回目标 | 不重新配对、不拆原连接，切回后再次收到三种全零探针 |
| 回连原槽位 | 输入目标为槽位 2 时，主动断开并重连第一台主机，仍认领原槽位 1，输入目标保持为 2 |
| 双机并发与跨主机隔离 | **待实测**；用户确认机器 2 当前不在 BT Switch 的蓝牙覆盖范围内，本轮不继续远程实测 |
| Boot/挂起/LED 写操作 | 核心状态测试通过；实际 D-Bus 写被本机 BlueZ 的 HID claimed-service 只读导出策略拒绝，未绕过或修改系统配置，未宣称实机通过 |
| USB→BLE | 仍未确认 Host VBUS，未验证完整输入链路 |

本次保护性 NVS 备份：`/tmp/opencode/bt-switch-rust-pre-multilink-nvs.bin`（64 KiB，权限 0600，包含潜在密钥，不提交）。

最新可重复单主机检查（明确不等同于双机并发）：

```sh
/usr/bin/python3 scripts/ble-single-host-check.py \
  --address 68:EE:8F:59:7F:36 \
  --log /tmp/opencode/bt-switch-multilink-final-single-host.json
```

实际结果：

```json
{
  "neutral_probe": true,
  "offline_target_preserves_connection_without_fallback": true,
  "return_without_repairing": true,
  "simultaneous_hosts_tested": false
}
```

测试脚本只发送全零探针，不合成按键或鼠标移动，并恢复原来的输入槽位。
观测端结束时会取消自身订阅，所以结束后的 `ready` 应以实际主机订阅状态为准。
新增日志位于 `/tmp/opencode/bt-switch-multilink-*.log` 及上述 JSON 对应的 `.serial.log`。

后续机器 2 回到蓝牙范围且 SSH 可用时，可重跑连接门槛检查：

```sh
/usr/bin/python3 scripts/ble-multilink-check.py \
  --remote duckrans-laptop --ssh-command 'tailscale ssh' \
  --address 68:EE:8F:59:7F:36 --power-on-remote \
  --log /tmp/opencode/bt-switch-multilink-two-hosts.json
```

仍需验证两条连接同时加密/订阅、两端观测定向探针、切换旧新目标释放、离线第三槽位及切换延迟。
连接门槛检查通过不能替代这些检查；旧 C 配对可能需要经用户另行授权后在机器 2 重新配对。

## 以下为早期单连接固件的历史记录

下文保留原始测试证据，诸如“容量 1”“切换会断开”“未接入多连接”仅描述升级前的固件，不代表当前实现。

## 硬件与保护措施

- 串口：`/dev/ttyUSB0`，CH340 USB-UART。
- 实读芯片：ESP32-S3 revision v0.2，晶振 40 MHz，Flash 16 MB。
- 用户提供的是第三方开发板图片，不是乐鑫官方 USB-OTG 板；默认不配置未知的供电、路由、LED 或按键 GPIO。
- 用户确认鼠标 2.4 GHz 接收器已接开发板，但尚无对外 5 V 供电；电脑上枚举到的另一个接收器是键盘接收器。
- 未擦除整个 Flash 或 NVS；Rust 正常运行会写入自己的槽位及 NimBLE 配对信息，不能据此保证旧 C 槽位格式仍可直接使用。

烧录前完整备份：

```text
/tmp/opencode/bt-switch-pre-rust-20261111-full.bin
长度：16777216 字节
SHA-256：a36fe7d42cf65ae6693b708103fae919885896dc0c6f808608b1b1d35331f894
```

备份包含潜在配对密钥，不能提交或公开；该目录为临时存储，长期留存应另行安全归档。
首次低速备份超时产生的空文件不算有效备份。

从备份读取到的实板分区与本次烧录表一致：

| 分区 | 起始地址 | 大小 |
| --- | --- | --- |
| NVS | `0x9000` | `0x10000` |
| PHY | `0x19000` | `0x1000` |
| factory | `0x20000` | `0x300000` |

## 结果

| 项目 | 结果与实际证据 |
| --- | --- |
| 编译和烧录 | release 构建、Clippy 通过；espflash 完成写入及默认校验 |
| 串口冒烟 | Rust 帮助和状态命令可用；离线切槽并恢复成功；约半分钟观察未发现 panic 日志 |
| 实际 BLE 广播 | Linux 扫描收到 `BT Switch` 的本次无线观测，RSSI −37 dBm，包含 HID UUID `0x1812`；不是仅读取缓存对象 |
| 旧配对回连 | 初次出现反复认证失败，不能当作已通过 |
| 新配对 | 经用户授权，仅删除当前 Linux 的该设备旧配对记录；使用临时、仅允许目标设备的 `NoInputNoOutput` 代理，新配对成功 |
| 加密与服务解析 | Linux：`Paired=true / Connected=true / ServicesResolved=true`；固件确认主机保存到槽位 1 且已加密 |
| 实际 HID 通知 | 显式订阅三个输入报告后发送串口 `t`，实际收到三份全零通知，报告 1/2/3 长度为 8/7/2 字节 |
| 配对持久化 | 为增加 `t` 探针重新烧录并重启后，不重新配对即可加密回连 |
| 单连接切槽与返回 | 切到空槽位 2 后 `connected=false ready=false`；返回槽位 1 后无需重新配对即可恢复加密连接 |
| USB 输入 | 状态为 `0 interfaces, 0 reports decoded`；Host VBUS 未确认，端到端输入未验证，不能归因为驱动故障 |
| 多主机同时在线 | **未实现、未验证**；当前仍是单连接，切槽会断开旧连接 |

初次认证失败在清理本机旧记录并使用临时代理重新配对后不再复现。
这不是对旧密钥兼容性的验证，也不能单凭这次测试区分旧密钥与原代理配置各自的影响。

`ready=true` 在当前固件中仍为加密后的 800 ms 计时状态，不代表逐连接 CCCD 门控已经实现。
通知通过的证据是实际接收到三份全零报告，而不是该状态字段。
没有合成实际键值、鼠标位移或滚轮，也未验证系统输入事件或 USB→BLE 完整链路。

测试结束时开发板保留 Rust 固件，选择槽位 1，与当前 Linux 配对并加密连接。

## 两机并发基线检查

同日经用户授权，通过 SSH 控制 `192.168.31.209` 的蓝牙进行并发基线测试。
本轮没有改动固件，也没有删除第二台电脑的旧 C 固件配对记录。

- 第一台：测试前后均为 `Paired=true / Connected=true / ServicesResolved=true`。
- 第二台：SSH 可用；蓝牙地址为 `A0:29:42:83:FB:A7`，最初蓝牙关闭，保留旧配对且未连接。
- 临时开启第二台蓝牙，在不切换输入目标、不主动断开第一台的情况下调用 `Device1.Connect`。
- 第二台调用等待 12 秒后返回 D-Bus `NoReply`；随后查询仍为 `Connected=false / ServicesResolved=false`。
- 固件串口状态：槽位 1 `connected=true ready=true secured=true advertising=false`。
- 结果：**同时在线检查不通过**。第二台保留旧配对本身不证明它已与 Rust 固件配对；D-Bus 超时本身也不能唯一定位协议失败原因。
- 静态实现证据：`CONFIG_BT_NIMBLE_MAX_CONNECTIONS=1`，仅一个 `Connection` 对象，选择槽位仍调用断开，连接期间不继续广播；当前固件不能满足真正多连接。
- 测试后取消第二台本次连接请求，恢复其蓝牙为关闭状态；第一台连接保留，未修改其他蓝牙设备或清理配对。

可重跑的并发门槛检查：

```sh
/usr/bin/python3 scripts/ble-multilink-check.py \
  --remote 192.168.31.209 --address 68:EE:8F:59:7F:36 \
  --power-on-remote --log /tmp/opencode/bt-switch-multilink-baseline-20261111.json
```

该检查明确要求第一台先在线；不会把第二台单独连接或 `Paired=true` 当成并发通过。
即使后续连接门槛通过，仍需单独验证两条连接的加密、订阅、定向通知、切换释放与延迟。
本轮串口记录：`/tmp/opencode/bt-switch-multilink-serial-20261111.log`。

## 可重复命令

串口冒烟在发现已有连接时跳过切槽，避免意外断开主机：

```sh
python3 scripts/board-smoke.py --port /dev/ttyUSB0 --log /tmp/opencode/board-smoke.log
```

以下脚本使用系统 Python 的 `dbus` 和 `gi`。扫描应在设备正在广播时运行；配对需先在串口为空槽位打开 `p` 窗口。
脚本不会删除旧配对或注册为系统默认配对代理。

```sh
/usr/bin/python3 scripts/ble-scan.py --address 68:EE:8F:59:7F:36
/usr/bin/python3 scripts/ble-pair.py --address 68:EE:8F:59:7F:36
/usr/bin/python3 scripts/ble-neutral-probe.py --address 68:EE:8F:59:7F:36 --port /dev/ttyUSB0
```

全零探针在未包含 `t` 命令的旧固件上实际运行结果为 `FAIL: received={}, errors=[]`，
增加探针并重新烧录后实际通过，证明检查能够检测“未收到通知”，而不是无条件成功。

串口日志：

- `/tmp/opencode/bt-switch-rust-smoke-20261111.log`
- `/tmp/opencode/bt-switch-rust-pair-20261111.log`
- `/tmp/opencode/bt-switch-rust-roundtrip-20261111.log`
