# ESP32-S3 蓝牙键鼠切换器

基于 PlatformIO + ESP-IDF 6.1，数据链路为：

```text
2.4GHz 键鼠接收器 → USB Host → HID 描述符解析 → BLE HID → 当前选中的电脑/平板
```

## 当前实现范围

- 默认 **3 个配对槽位**，保存配对关系和上次选择，支持断电后恢复。
- **默认同时保持最多 3 台主机连接**，只有选中的主机接收键鼠输入。按钮/串口切换仅改变输入目标，不断开其他主机。
- 每条连接独立维护加密、HID 通知订阅（CCCD）、协议模式和挂起状态。所有已配对主机均可回连，不再只允许当前输入槽位连接。
- USB 复合 HID 接口、Report ID、标准键盘、常见 NKRO 位图、鼠标按钮/相对位移/滚轮/横向滚轮、常见多媒体键。
- USB 输入侧按描述符解析位宽，不假定所有接收器都发送固定的 boot 报告。
- BLE 输出：6 键无冲键盘 + 修饰键、8 按钮鼠标、16 位 XY 位移、8 位滚轮/横滚、单个 Consumer Usage。
- 按钮消抖，短按轮换，长按 3 秒打开空槽位配对窗口；也能通过串口控制。
- USB 拔出、传输错误、输入队列溢出时清除对应输入状态；切换后需先松开按键，才向新主机转发输入。

**状态：已通过固件编译、解析器/多连接路由测试，以及两台 Linux 电脑同时连接、加密、订阅、轮换输入目标的上板检查。** 第二台电脑的 HCI 抓包验证了通知只发向输入目标。USB 接收器到 BLE 的完整输入链路、第三台主机、其他操作系统及随机私有地址主机尚未完整实测。

## 烧录前确认硬件

当前 `platformio.ini` 保留你创建项目时选择的 `esp32s3usbotg`，它是**乐鑫官方板配置，不代表你的第三方板一定匹配**。

1. 确认芯片确实为 ESP32-S3、Flash 大小及模式与板配置匹配；当前板配置宣称 8MB Flash。
2. 确认 Host Type-C 的数据线连接原生 USB：GPIO19=D−、GPIO20=D+。不要给这两个引脚接按钮。
3. 确认 Host 接口有稳定的 5V VBUS。若有电源开关、使能 GPIO、USB 路由/角色控制引脚，应按该板原理图配置；本工程不会猜测官方 OTG 板的电源/路由引脚。
4. 用**串口 Type-C**烧录及查看日志；Host Type-C 用转接头连接接收器。选用支持该板 Host 角色的转接头，不要只依据接口外形判断。
5. 不要连接两个主机供电口造成 5V 反灌；连接方式以原理图/厂家说明为准。

默认不启用 PSRAM，不驱动未知 USB 供电 GPIO，也不配置未知按钮 GPIO。

## 配置按钮和 USB 供电

运行 PlatformIO 的 `menuconfig` 任务，或在 PlatformIO 终端执行：

```sh
pio run -t menuconfig
```

在 **Bluetooth switch** 菜单中设置：

| 配置项 | 默认值 | 用途 |
| --- | --- | --- |
| `SWITCH_SLOT_COUNT` | 3 | 配对槽位数，2–6 |
| `SWITCH_BUTTON_GPIO` | -1 | 按钮引脚，-1 禁用 |
| `SWITCH_VBUS_GPIO` | -1 | Host 5V 使能引脚，-1 不控制 |
| `SWITCH_VBUS_ACTIVE_LEVEL` | 1 | VBUS 使能有效电平 |
| `SWITCH_PAIRING_SECONDS` | 120 | 空槽位配对窗口时长 |

蓝牙配置已启用 `BT_MULTI_CONNECTION_ENBALE`（ESP-IDF 的原始拼写），设置 `BT_ACL_CONNECTIONS=6`、`BT_CTRL_BLE_MAX_ACT=8`。实际允许在线数由 `SWITCH_SLOT_COUNT` 限制；编译时会检查至少为广播保留一个控制器活动名额。

按钮为**低电平有效**：GPIO 与 GND 之间接常开按钮，程序启用内部上拉。若确认板载 BOOT 按钮接 GPIO0，可以设置 `SWITCH_BUTTON_GPIO=0`；不要在上电/复位时按住，否则会进入下载模式。避免使用 Flash、PSRAM、串口及其他板载外设占用的引脚。

配置保存在 `sdkconfig.esp32s3usbotg`；`sdkconfig.defaults` 用于新配置初始化，修改 defaults 不保证覆盖已有配置。

## 编译、烧录、监视

```sh
pio run
pio run -t upload
pio device monitor -b 115200
```

如有多个串口，在命令中加 `--upload-port /dev/ttyUSB0` 或监视器的 `--port /dev/ttyUSB0`，替换为实际端口。PlatformIO 会根据 `src/idf_component.yml` 自动下载 Espressif USB Host HID 组件；保留 `dependencies.lock` 锁定依赖版本。

分区表为 `partitions.csv`，包含 64KB NVS 和 3MB 应用区，无 OTA。**分区布局与初始化工程不同**；如果此前已有其他固件/NVS 数据，应先备份，再决定是否擦除。程序不会自动清空 NVS 或旧配对。

若 Linux 报 `/dev/ttyUSB0: Permission denied`，检查设备所属组和当前用户的组成员关系。本机串口属于 `uucp`，已将用户 `duckran` 加入该组；需注销并重新登录，使 VS Code 等进程获得新组权限。当前连接已另行授予临时 ACL，重新插拔后该临时权限可能消失。无需以 root 运行 PlatformIO。

本次启动检测到实际 Flash 为 16MB；当前沿用 8MB 镜像配置，仅使用其中 8MB，不影响现有分区运行。若后续需要使用全部容量，应同步修改板配置和 ESP-IDF Flash 大小设置。

## 第一次测试

1. 先不插接收器，打开串口，看到帮助信息后发送 `s`。默认按钮禁用时使用串口即可。
2. 启动时若选中的是空槽位，会开放 120 秒配对窗口。在第一台电脑的蓝牙设置中找到 **BT Switch** 并配对。
3. 观察 `Paired host saved in slot 1` 和 `Slot 1 ready`。打开空白文本编辑器，再插上接收器，按键/移动鼠标。
4. USB 日志应出现 VID/PID、interface、fields、reports。发送 `s`，输入时 `decoded reports` 应增加。若没有枚举日志，优先检查 VBUS、转接头和 USB 路由。
5. 发送 `n` 或短按按钮切到槽位 2，在第二台电脑上配对。**电脑 1 保持连接，不必关闭蓝牙。** 槽位 3 同理；配对期间输入仍只路由到选中的槽位。
6. 再发送 `1` 或轮换回槽位 1。在线主机间切换不触发断开/重连。发送 `s`，应看到两个槽位 `connected=1 encrypted=1 notify=0x07`，以及 `links=2/3 mode=simultaneous`；切换前后 `conn` 不变。
7. 验证按住 Ctrl、鼠标按键时切换，松开后新主机不应收到旧的按下状态；拔掉接收器时不应留下持续按下的键。

串口命令逐字符处理，不要求回车；若终端不立即发送，可按回车。

| 命令/操作 | 功能 |
| --- | --- |
| `n` / 按钮短按 | 下一个槽位，包括空槽位 |
| `1`…`3` | 直接选择槽位（范围由配置决定） |
| `p` / 按钮长按 3 秒 | 重新开放当前空槽位的配对窗口，不覆盖已有配对 |
| `s` | 输出槽位、连接、USB 输入计数 |
| `t` | 只向当前输入目标发送全释放的中性 HID 报告，用于验证传输；不按键、不移动指针 |
| `h` / `?` | 帮助 |
| **`X`** | **删除当前槽位配对并重新开放配对窗口** |

配对失效时：选择对应槽位，发送大写 `X`，同时在该主机上“忘记/删除 BT Switch”，再重新配对。`X` 只断开、清除该槽位，不删除其他槽位或断开其他已连接主机。不要仅删除一端的记录。同一台主机只分配一个槽位，蓝牙名称始终为 `BT Switch`。

升级保留原有蓝牙地址、NVS 槽位布局、报告描述符和 GATT 句柄顺序。若此前认证超时已导致蓝牙栈删除密钥，需要重新配对对应槽位；不要无差别擦除所有槽位。

Linux 首次配对若失败为 `0x59`，**不要直接认定是密钥损坏**。先检查 `journalctl -u bluetooth`；若出现 `No agent available for request type 2`，应先恢复电脑端配对代理。第二台电脑的已验证诊断见 [docs/second-host-pairing.md](docs/second-host-pairing.md)。

## 已知限制

- ESP32-S3 不支持经典蓝牙；主机必须支持 BLE HID。
- 在线主机间切换不重新建立连接；若目标主机离线/休眠/挂起或未订阅通知，则暂停输入，不自动切到其他槽位。首次连接或意外断线后的回连时机仍由主机系统决定。
- 使用 Secure Connections bonding、无显示/键盘能力的 Just Works 配对，无 MITM 验证。只在可信环境开放配对；窗口期间附近设备可能抢先配对。
- 就绪状态依据该连接的加密与真实 CCCD 订阅，而不是固定等待 800ms。CCCD 按配对槽位保存，报告模式和 boot 模式分别处理。
- USB 5V 供电和第三方板的电源/路由 IO 尚未确认。默认 `USB_HOST_HUBS_SUPPORTED` 未开启；如需经过外置 Hub，除了供电，还需要在 USB Host 配置中启用 Hub 支持并单独验证。当前建议先直连接收器。
- 不支持任意厂商私有接收器协议、非连续选择列表的 HID 数组、绝对坐标设备、手柄、音频和系统控制键。复杂描述符超出解析器容量会拒绝，而不是猜测布局。
- 输入端可以解析 NKRO，但 BLE 输出仍为 6 键无冲，超出时发送标准 ErrorRollOver。
- 多媒体键一次只输出一个 usage；键盘 Caps Lock/Num Lock 等 LED 输出尚未回传 USB 接收器。
- 鼠标超出 BLE 报告范围的单包位移会饱和裁剪。拥塞/发送失败时不保证每一包相对位移都被保留；键盘/按钮状态会重试。
- 当前最多处理 8 个受支持的 HID 接口。描述符解析不会主动请求接收器的厂商专有模式。
- 改变槽位数量会使应用槽位记录格式不匹配。应在改变配置前清除已有槽位和各主机配对，不要依靠迁移旧记录。

## 本机测试

安装本机 C 编译器和 Python 3 后：

```sh
python3 test/run_host_tests.py
```

测试使用 AddressSanitizer/UndefinedBehaviorSanitizer，覆盖键盘修饰键/数组、输入与输出偏移、232-bit NKRO、Report ID、12-bit 打包有符号鼠标位移、滚轮、多媒体键、截断/非法描述符和 20,000 组确定性模糊输入；还覆盖同时在线路由、释放旧/新目标、拥塞重试、离线目标、重复连接、逐连接订阅/挂起和加密门控。

烧录后可使用安装了 `pyserial` 的 Python 执行非破坏性的串口启动检查（先关闭其他串口监视器）：

```sh
~/.platformio/penv/bin/python test/check_device_boot.py --port /dev/ttyUSB0
```

检查会发送 `s` 查询状态；若观察到崩溃/重启或没有应用状态响应则失败。它不主动控制复位、不删除配对，也不验证主机实际配对或 USB 转发。部分 CH340 开发板会在打开串口时复位，需要等待回连。

两台已配对电脑的多连接硬件回归（电脑 2 为 `192.168.31.209`，需要 SSH 和 btmon 权限）：

```sh
~/.platformio/penv/bin/python test/check_multilink.py --cycles 5
```

它检查槽位 1/2 同时在线并保持原连接 ID、连续切换、选中离线槽位仍保留其他连接，以及 `t` 中性报告的逐主机计数隔离；电脑 2 的 HCI 抓包额外验证只在槽位 2 选中时收到测试通知。最后会故意断开/重连电脑 2 的 BT Switch，验证不影响电脑 1；可用 `--skip-reconnect` 跳过。不会注入按键或鼠标位移，不删除配对。详见 [docs/multilink.md](docs/multilink.md)。

## 代码结构

- `src/main.c`：串口、按钮、输入合并及切换后的松键保护。
- `src/usb_input.c`：USB Host、接口生命周期、各 Report ID 状态合并。
- `src/hid_parser.c`：独立于 ESP-IDF 的有界 HID 描述符与输入报告解析。
- `src/ble_switch.c`：多连接 GATT HID、配对槽位、逐连接 ATT 状态及所有已配对主机的回连广播。
- `src/hid_router.c`：独立于 ESP-IDF 的多主机输入路由、加密/订阅门控及拥塞释放重试。
- `src/hid_profile.h`：HID 报告描述符和兼容旧固件的 GATT 属性布局。
- `src/Kconfig.projbuild`：引脚、槽位数及配对窗口配置。
