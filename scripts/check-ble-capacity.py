#!/usr/bin/env python3
"""检查实际 NimBLE C 编译命令，不能仅凭 MAX_CONNECTIONS 推断 SMP 容量。"""

import argparse
import json
import re
import shlex
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", required=True, help="当前 esp-idf-sys 的 out/build 目录，含 compile_commands.json")
    args = parser.parse_args()
    build = Path(args.build_dir)
    config = (build.parent / "sdkconfig").read_text()

    def value(name):
        match = re.search(rf"^{re.escape(name)}=(\d+)$", config, re.M)
        if not match:
            raise RuntimeError(f"缺少配置 {name}")
        return int(match[1])

    connections = value("CONFIG_BT_NIMBLE_MAX_CONNECTIONS")
    activities = value("CONFIG_BT_CTRL_BLE_MAX_ACT")
    cccds = value("CONFIG_BT_NIMBLE_MAX_CCCDS")
    rows = json.loads((build / "compile_commands.json").read_text())
    security = [row for row in rows if row["file"].endswith("/host/src/ble_sm.c")]
    if not security:
        raise RuntimeError("未找到真实 NimBLE ble_sm.c 的编译命令")

    for row in security:
        command = row.get("arguments") or shlex.split(row["command"])
        defines = [part for part in command if part.startswith("-DMYNEWT_VAL_BLE_SM_MAX_PROCS=")]
        if len(defines) != 1:
            raise RuntimeError("SMP 容量未显式覆盖，ESP-IDF 6.1 默认仅为 1")
        procedures = int(defines[0].split("=", 1)[1])
        if procedures < connections or activities < connections + 1 or cccds < connections * 7:
            raise RuntimeError("连接、SMP、广播活动或 CCCD 容量不足")
    print(f"PASS: connections={connections}, SMP={procedures}, activities={activities}, CCCDs={cccds}")


if __name__ == "__main__":
    main()
