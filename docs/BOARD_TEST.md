# Rust 固件上板测试记录

日期：2026-11-11。测试对象为当前 Rust 工作树，包含 BLE 职责拆分、通用板配置和串口全零探针。

## 最新：多连接实现接入后的回归

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
