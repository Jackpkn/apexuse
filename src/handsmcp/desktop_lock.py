"""Cooperate with other handsmcp processes using a local advisory lock."""
import asyncio
from contextlib import asynccontextmanager
import fcntl
import os
from pathlib import Path

class Lease:
    def __init__(self, fd): self.fd = fd

    def generation(self):
        os.lseek(self.fd, 0, os.SEEK_SET)
        data = os.read(self.fd, 64)
        if not data:
            return 0
        if not data.isdigit():
            raise OSError('invalid desktop lock generation')
        return int(data)

    def advance(self):
        data = str(self.generation() + 1).encode('ascii')
        os.lseek(self.fd, 0, os.SEEK_SET)
        os.ftruncate(self.fd, 0)
        if os.write(self.fd, data) != len(data):
            raise OSError('cannot update desktop lock generation')

class DesktopLock:
    def __init__(self, path, timeout=10):
        self.path = Path(path)
        self.timeout = timeout

    @asynccontextmanager
    async def hold(self):
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        fd = os.open(self.path, os.O_RDWR | os.O_CREAT | getattr(os, 'O_NOFOLLOW', 0), 0o600)
        locked = False
        try:
            os.fchmod(fd, 0o600)
            deadline = asyncio.get_running_loop().time() + self.timeout
            while True:
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    locked = True
                    break
                except BlockingIOError:
                    if asyncio.get_running_loop().time() >= deadline:
                        raise TimeoutError('desktop lock timeout')
                    await asyncio.sleep(0.05)
            yield Lease(fd)
        finally:
            if locked:
                fcntl.flock(fd, fcntl.LOCK_UN)
            os.close(fd)
