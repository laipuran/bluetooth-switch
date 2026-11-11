"""观测端的进程适配器：请求/应答只通过 JSON 行，不依赖主机内存或时钟同步。"""

import json
import select
import subprocess


class ObserverClient:
    def __init__(self, command):
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            if self.receive() != {"ready": True}:
                raise RuntimeError("观测端未就绪")
        except Exception:
            self.close()
            raise

    def receive(self):
        if not select.select([self.process.stdout], [], [], 10)[0]:
            raise TimeoutError("观测端响应超时")
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError("观测端已退出")
        value = json.loads(line)
        if "error" in value:
            raise RuntimeError(value["error"])
        return value

    def request(self, action, **values):
        self.process.stdin.write(json.dumps({"action": action, **values}) + "\n")
        self.process.stdin.flush()
        return self.receive()

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=5)
