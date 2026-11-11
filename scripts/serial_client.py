"""串口测试适配器：不切换 DTR/RTS，记录原始日志并恢复串口配置。"""

import os
import select
import termios
import time


class SerialClient:
    def __init__(self, port, log):
        self.fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        self.original = termios.tcgetattr(self.fd)
        config = termios.tcgetattr(self.fd)
        config[0] = 0
        config[1] = 0
        config[2] = termios.CS8 | termios.CLOCAL | termios.CREAD
        config[3] = 0
        config[4] = termios.B115200
        config[5] = termios.B115200
        termios.tcsetattr(self.fd, termios.TCSANOW, config)
        self.log = open(log, "wb")

    def write(self, command):
        sent_at = time.monotonic()
        os.write(self.fd, command.encode("ascii"))
        return sent_at

    def collect(self, seconds):
        deadline = time.monotonic() + seconds
        chunks = []
        while time.monotonic() < deadline:
            readable, _, _ = select.select([self.fd], [], [], max(0, deadline - time.monotonic()))
            if readable:
                try:
                    data = os.read(self.fd, 8192)
                except BlockingIOError:
                    continue
                if not data:
                    raise RuntimeError("串口已断开")
                chunks.append(data)
                self.log.write(data)
                self.log.flush()
        return b"".join(chunks).decode("utf-8", errors="replace")

    def exchange(self, command, seconds=0.5):
        self.write(command)
        return self.collect(seconds)

    def close(self):
        try:
            termios.tcsetattr(self.fd, termios.TCSANOW, self.original)
        finally:
            os.close(self.fd)
            self.log.close()
