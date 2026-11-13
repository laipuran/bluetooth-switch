# 设计说明

## 1. 数据通路

```text
USB 接收器
  │  USB interrupt IN
  ▼
usb_host_hid（ESP-IDF C 组件）
  │  原始 report + report ID
  ▼
usb.rs / InterfaceCtx
  │  Parser::decode -> 每个 report ID 的 Decoded
  │  合并同一 interface 的所有 report ID
  ▼
mpsc channel（有界，128）
  ▼
app/input.rs
  │  Translator::update（按 interface 合并键盘/按钮/多媒体）
  ▼
Translator::flush
  │  keyboard / mouse / consumer 三个 report
  ▼
ble/mod.rs → core/router → ble/transport.rs
  │  NimBLE notify_with(data, connection)
  ▼
当前选中的 BLE 主机
```

### 应用协调与诊断模块

`main.rs` 只初始化日志并进入 `app::run()`。`app/mod.rs` 维护处理顺序和调度；
`app/controls.rs` 将按键与串口归一为相同操作；`app/input.rs` 隐藏输入合并、
溢出释放、切换保护及发送确认。结构整理阶段仍保留固定 4 ms 等待，不混入调度优化。

通用 USB 观测独立于 HID：`usb/discovery.rs` 处理客户端与扫描，
`usb/discovery/describe.rs` 只读缓存描述符，`usb/discovery/port.rs` 只读端口与 PHY。
`usb-gpio-check` 二进制复用观测端，不启动 BLE/HID；GPIO 原始输入值不能替代 USB PHY 线路状态。
BLE 的 `link_params.rs` 只读实际连接参数，不提交更新请求；状态日志不把请求值当作协商值。

### 为什么按 report ID 分别保存状态

一个 HID interface 可能用不同的 report ID 分别发送键盘、鼠标和 Consumer 数据。C 版本的 `usb_input.c` 为每个 report layout 保存一份 `hid_input_t`，每次收到数据后把它们合并，再把“当前完整状态 + 本次相对位移”交给主循环。Rust 版保持同样做法：

* `states[index]` 保存每个 layout 的按键/按钮/多媒体键。
* `merged.merge_state()` 对按键位图、鼠标按钮和 Consumer 做并集。
* 相对位移（X/Y/Wheel/Pan）只取当前数据包，绝不从 `states` 重放。

这样键盘 report 和 Consumer report 交替到达时不会互相清空状态。

## 2. 切换时的输入保护（armed）

切换主机时最危险的是把“按住的 Ctrl/Alt/鼠标键”带到新主机。`Translator` 有一个显式的 armed 状态：

```text
BLE not ready ──> disarm
                     │
BLE ready ──> 等待所有 source 的 keys/buttons/consumer 归零
                     │
                     ▼
             发送三份全零 report
                     │
             已订阅的全释放成功排队
                     ▼
                  armed
```

* 未 armed 时只更新 source 状态，不累加相对位移。
* armed 后 `flush()` 只在键盘/多媒体状态变化时发送；鼠标位移按主循环 tick 聚合。
* `send_reports()` 返回 `Sent`，只有 NimBLE 接受通知排队才确认；这不是无线交付确认。未排队的位移保留到下个 tick。
* 路由器先对旧、新目标已订阅的报告排队全释放；`Translator` 在物理中立后从成功排队的报告重新 arm，不要求主机订阅所有报告。
* 切换、忘记及输入目标的连接/订阅状态变化都会设置 `input_reset`。即使断连和回连在一次 `poll` 内完成，主循环仍会 `disarm()`，防止 held keys 沿用旧状态。

## 3. BLE 状态机

### 模块职责

| 路径（相对 `crates/firmware/src/ble/`） | 职责 |
| --- | --- |
| `mod.rs` | 对外入口、初始化、轮询协调与输入就绪门控 |
| `connection.rs` | 每条连接的安全握手、配对候选、断开重试与认证超时 |
| `admission.rs` | 消费连接事件、校验主机身份、接纳或拒绝配对 |
| `management.rs` | 用户命令、槽位切换、配对窗口、忘记主机和持久化协调 |
| `advertising.rs` | 广播数据、白名单、控制器启停及重试状态 |
| `bond.rs` | 删除单个 bond 前暂停广播，处理已删除记录，不拆其他连接 |
| `events.rs` | NimBLE 回调注册、事件投递和地址转换 |
| `profile.rs` | GATT 布局及加密权限，Boot 属性追加到已有 HID 报告后 |
| `attributes.rs` | 回调访问的逐连接属性快照，短锁内只操作内存 |
| `transport.rs` | 按句柄定向通知、Boot 鼠标转换、成功排队后的读取快照 |
| `transport/callbacks.rs` | 逐连接读写及真实 CCCD 订阅事件 |

`Advertising`、`Connection` 和 `HidTransport` 各自持有本职责的状态；
`BleManager` 协调连接集合与 `Router`。回调可更新逐连接属性快照，但只通过事件通知管理器，不修改槽位、不写 NVS、不调用管理命令。

核心路由器已接入固件：`mod.rs` 管理连接与授权，`delivery.rs` 负责选择、释放重试和投递，`tests.rs` 以记录通知的适配器验证行为。
另有 `session.rs` 管理每条连接的协议、Report/Boot 订阅、挂起、LED 与读取值。

当前默认 `MAX_CONNECTIONS=3`、控制器活动数为 4（保留一个广播活动）、CCCD 存储容量为 48。不能只修改槽位数而保持底层容量不变。

**另有独立的 SMP 过程池容量。** ESP-IDF v6.1 的 `esp_nimble_cfg.h` 默认
`MYNEWT_VAL_BLE_SM_MAX_PROCS=1`，不会随 `MAX_CONNECTIONS` 自动提升。
两台中心设备可以自行同时发起加密，应用串行请求不能保证底层只需一个状态机。
`components/ble_capacity/CMakeLists.txt` 给真实 `bt` 构建目标定义过程池容量等于连接数。
`scripts/check-ble-capacity.py` 检查 `ble_sm.c` 的实际编译命令；不能只检查 Rust 常量或 sdkconfig。

### 每条连接的独立状态

```text
connected → 身份匹配或配对候选 → 加密授权 → 有效协议下的实际通知订阅
    │                                         │
    └─ 认证失败/30 s 超时 → closing             ▼
                                   全释放成功排队且未挂起
                                              │
                                    若为输入目标，允许转发
```

* **准入**：已配对主机根据 identity address 认领原槽位；输入目标可以离线。同槽位的重复连接被拒绝。
* **新配对**：建连时捕获空槽位与窗口，认证时还需窗口有效且候选未撤销；切换输入目标不能把认证中的设备写入另一个槽位。仅允许一个未知主机等待认证。
* **安全请求**：应用侧串行发起，暂时失败每 500 ms 重试；NimBLE 自身也会处理加密属性触发的安全请求。30 s 未授权则仅关闭该连接。
* **真实就绪**：没有 800 ms 延迟。`Router::ready` 要求目标存在、加密、未挂起、至少一个当前协议下的 CCCD 订阅，以及释放欠账为零。
* **广播**：连接未满即继续广播；白名单包含所有已配对槽位。配对窗口允许未知设备，已有未知握手时暂时仅允许已配对主机。先停止广告再更新白名单，以栈实际广播状态处理启停竞争。
* **切换**：同步改变目标并尝试旧、新目标全释放，后续轮询重试失败项；旧目标拥塞不阻塞新目标。没有自动回退或断开动作。
* **忘记**：捕获要忘记的槽位，只关闭其连接；等待实际断开后暂停广播，再删除密钥和槽位。ESP-IDF v6.1 在广播活跃时拒绝 `ble_gap_unpair`；忘记期间暂不接受新连接，完成后恢复广播，但其他已建立的连接始终保留。删除失败限速重试，不因后来改变输入目标而误删新目标。
* **断开**：每 500 ms 重试请求，不在 5 s 后伪造断开或释放容量；避免控制器仍持有连接时复用句柄。
* **动态属性**：读回调为请求连接生成值，写回调只更新该连接；通知从不使用共享 characteristic 值广播。相对位移不保留到读取快照。

实现与验证分开：现已上板验证两台 Linux 同时加密与订阅、定向通知、旧新目标释放、离线目标、单槽位忘记及重复复位恢复。第三台、其他系统、RPA 和 USB 输入未验证，见 `BOARD_TEST.md`。

## 4. 槽位持久化

NVS namespace `bt_switch`，key `slots`，固定 53 字节：

Flash 分区与当前 C 工程对齐：NVS 为 `0x9000..0x18fff`，PHY 为 `0x19000`，
应用起始地址为 `0x20000`。烧录不擦除 NVS；旧 C 槽位结构与 NimBLE 配对存储不保证兼容，
保留原始数据不代表 Rust 可直接继承全部配对关系。

| 偏移 | 长度 | 内容 |
| --- | --- | --- |
| 0 | 4 | version = 1 (LE) |
| 4 | 1 | selected slot |
| 5 + 8·i | 1 | used |
| 6 + 8·i | 1 | BLE address type |
| 7 + 8·i | 6 | address (NimBLE little-endian) |

最多 6 个槽位（`bt_switch_core::MAX_SLOTS`），固件默认使用 3 个。任何字段不一致都会回退到空槽位表，而不是擦除整个 NVS。

## 5. 按键

`control::Button` 是一个纯逻辑、按 tick 驱动的消抖器：

* 原始电平变化后稳定 15 ms 才接受。
* 按下后持续 `long_ms` 发一次 `LongPress`。
* 释放时若未触发长按，则发 `Click`。
* 长按触发后，释放不再产生 Click。

默认映射见 README。要换成旋钮/编码器时，只需要把 `main.rs` 里的 `Button` 替换成编码器状态机，产生同样的 `Command`。

## 6. 线程模型

| 线程/任务 | 来源 | 职责 |
| --- | --- | --- |
| `main` | esp-idf-svc | 状态机 tick、按键、USB 事件消费、report 转发 |
| `usb_lib` | `std::thread` | `usb_host_lib_handle_events`、设备释放 |
| `usb_connect` | `std::thread` | HID interface open/start/close、描述符解析、上下文注册 |
| HID 后台任务 | `hid_host_install(create_background_task=true)` | 枚举/input report 回调，只做解码和非阻塞投递 |
| NimBLE host task | ESP-IDF | GAP/GATT/安全回调 |
| `console` | `std::thread` | 阻塞读取 UART 命令 |

共享状态：

* `TX`：USB 回调 -> 主循环的有界 channel。
* `LIFECYCLE_TX`：HID driver -> `usb_connect` 线程的连接/断开事件。
* `INTERFACES`：`Mutex<Vec<InterfaceCtx>>`，`usb_connect` 线程注册/移除，HID 回调解码。
* `OVERFLOW`：channel 满时置位；主循环清空 Translator 并发送全零 report。
* BLE 回调更新短锁保护的 `Sessions` 快照并投递事件；连接集合、槽位、路由器及 NVS 均由主循环管理。调用 NimBLE 前不持有 `Sessions` 锁，避免与特征锁形成反向锁顺序。

## 7. 板级细节

默认 `board/generic.rs` 不初始化未确认的 GPIO，仅提供空的 LED/按键接口，管理操作走串口。
当前上板目标为第三方 ESP32-S3，串口检测 Flash 为 16 MB；PSRAM 未启用，也不据模块图片推断其实际可用性。
USB Host 仍使用原生 GPIO19/20，接口路由和 VBUS 必须由实际硬件确认。

以下内容仅在 `--features official-otg` 时适用，代码位于 `board/official_otg.rs`。

ESP32-S3-USB-OTG 的 USB 口经过一个模拟开关：

* `USB_SEL=1` 时，GPIO19/20 接 Type-A HOST 口；`USB_SEL=0` 时接 USB_DEV 口。
* 固件默认 `DEV_VBUS_EN=1`、`BOOST_EN=0`，从 USB_DEV 取 5 V 给 HOST 口。
* `LIMIT_EN=1` 启用 500 mA 限流。

因此官方板烧录必须走 Micro-USB UART。若使用电池，把 `HOST_POWER` 改为 `BatteryBoost`。

## 8. 失败处理

| 情况 | 处理 |
| --- | --- |
| HID descriptor 不支持 | 关闭该 interface，记录日志，其他 interface 继续 |
| report 解码失败 | 记录前 3 次，忽略该包 |
| USB 传输错误 | 清零该 interface 的 layout 状态并通知 Translator 释放 |
| USB 事件队列满 | 丢弃事件、置 overflow、全量释放按键 |
| BLE 认证超时 | 断开 |
| 断开事件丢失 | 保留容量并重试断开，不伪造离线 |
| BLE 通知排队失败 | 保留释放欠账或未确认输入，下次轮询重试 |
| 选择离线槽位 | 保留其他连接并暂停输入，不回退 |
| NVS 数据损坏 | 使用空槽位表，不擦除 NVS |
| 切换时按键未释放 | 等全部释放后再 arm |
