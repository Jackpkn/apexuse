import pytest
from handsmcp.app_launch import launch_application

@pytest.fixture
def anyio_backend(): return 'asyncio'

@pytest.mark.anyio
async def test_launcher_uses_exact_argument_vector_and_checks_exit(monkeypatch):
    calls=[]
    class Process:
        async def wait(self): return 1
    async def spawn(*args,**kwargs): calls.append(args); return Process()
    monkeypatch.setattr('handsmcp.app_launch.asyncio.create_subprocess_exec',spawn)
    result=await launch_application('com.apple.calculator')
    assert calls == [('/usr/bin/open','-b','com.apple.calculator')]
    assert result == {'ok':False,'code':'launch_failed'}
