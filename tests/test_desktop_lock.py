import asyncio
import pytest
from handsmcp.desktop_lock import DesktopLock

@pytest.mark.anyio
async def test_serializes_separate_lock_instances(tmp_path):
    path = tmp_path / 'desktop.lock'
    one, two = DesktopLock(path), DesktopLock(path)
    entered = asyncio.Event()
    async with one.hold():
        async def worker():
            async with two.hold(): entered.set()
        task = asyncio.create_task(worker())
        await asyncio.sleep(0.05)
        assert not entered.is_set()
    await asyncio.wait_for(task, 1)
    assert entered.is_set()

@pytest.mark.anyio
async def test_generation_shared_between_instances(tmp_path):
    path = tmp_path / 'desktop.lock'
    async with DesktopLock(path).hold() as lease:
        assert lease.generation() == 0
        lease.advance()
    async with DesktopLock(path).hold() as lease:
        assert lease.generation() == 1

@pytest.fixture
def anyio_backend(): return 'asyncio'

@pytest.mark.anyio
async def test_lock_is_shared_across_processes(tmp_path):
    import sys
    path = tmp_path / 'desktop.lock'
    signal = tmp_path / 'acquired'
    script = ('import fcntl,os,sys; '
              'fd=os.open(sys.argv[1], os.O_RDWR|os.O_CREAT, 0o600); '
              'fcntl.flock(fd,fcntl.LOCK_EX); '
              'open(sys.argv[2],"w").write("acquired"); '
              'fcntl.flock(fd,fcntl.LOCK_UN); os.close(fd)')
    async with DesktopLock(path).hold():
        process = await asyncio.create_subprocess_exec(sys.executable,'-c',script,str(path),str(signal))
        await asyncio.sleep(0.15)
        assert not signal.exists()
    assert await asyncio.wait_for(process.wait(), 2) == 0
    assert signal.read_text() == 'acquired'
