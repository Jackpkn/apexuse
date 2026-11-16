"""Launch a previously validated macOS bundle ID without invoking a shell."""
import asyncio

async def launch_application(bundle):
    try:
        process = await asyncio.create_subprocess_exec(
            '/usr/bin/open', '-b', bundle,
            stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL,
        )
        try:
            status = await asyncio.wait_for(process.wait(), timeout=15)
        except TimeoutError:
            process.kill()
            await process.wait()
            return {'ok':False, 'code':'launch_timeout'}
    except OSError:
        return {'ok':False, 'code':'launch_failed'}
    if status != 0:
        return {'ok':False, 'code':'launch_failed'}
    return {'ok':True, 'message':'Launch requested; inspect the application to verify it opened.'}
