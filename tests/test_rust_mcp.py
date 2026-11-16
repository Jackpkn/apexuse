import os
from pathlib import Path
import pytest
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[1]
RUST_BIN = ROOT / "rust/target/release/handsmcp-mcp"


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.mark.anyio
async def test_rust_mcp_stdio_end_to_end(tmp_path):
    if not RUST_BIN.is_file():
        pytest.skip("Rust release binary not built")
    confirm_policy = tmp_path / "confirm_policy.yaml"
    confirm_policy.write_text("mode: normal\nconfirmation: required\nallowed_apps: [\"*\"]\n")
    env = dict(os.environ, HANDSMCP_STATE_DIR=str(tmp_path / "state"), HANDSMCP_POLICY=str(confirm_policy))
    params = StdioServerParameters(command=str(RUST_BIN), args=["--mock"], env=env)
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools = await session.list_tools()
            names = {t.name for t in tools.tools}
            assert names == {
                "inspect_app",
                "visual_inspect",
                "computer",
                "desktop",
                "screenshot",
                "workflow_memory",
            }

            # 1. List running apps
            apps = await session.call_tool("desktop", {"action": "list_applications", "arguments": {}})
            assert apps.structuredContent["ok"] is True
            assert any(a["bundle_id"] == "com.apple.calculator" for a in apps.structuredContent["apps"])

            # 2. Record workflow memory & verify inspect_app returns hints
            rec = await session.call_tool(
                "workflow_memory",
                {
                    "action": "record",
                    "app": "Calculator",
                    "goal": "Press seven",
                    "steps": [{"action": "click", "target_role": "Button", "target_label": "7", "note": "Numeric key"}],
                },
            )
            assert rec.structuredContent["ok"] is True

            insp = await session.call_tool("inspect_app", {"app": "Calculator"})
            assert insp.structuredContent["ok"] is True
            assert len(insp.structuredContent["workflow_hints"]) == 1

            # 3. Set-of-Mark visual_inspect returns both annotated image & fused mark index
            vis = await session.call_tool("visual_inspect", {"app": "Calculator"})
            assert vis.structuredContent["ok"] is True
            assert vis.structuredContent["marks_count"] == 3
            assert any(c.type == "image" and c.mimeType == "image/png" for c in vis.content)

            # 4. Visual click by Set-of-Mark ID requires confirmation, then executes on confirm
            clk = await session.call_tool("desktop", {"action": "visual_click", "arguments": {"mark": "m2"}})
            assert clk.structuredContent["code"] == "confirmation_required"
            tok = clk.structuredContent["token"]

            done = await session.call_tool("computer", {"action": "confirm", "arguments": {"token": tok}})
            assert done.structuredContent["ok"] is True
            assert done.structuredContent["method"] == "mock_ok"

            # 5. Desktop-wide Set-of-Mark visual_inspect + drag + type by mark
            desk = await session.call_tool("visual_inspect", {"app": "desktop"})
            assert desk.structuredContent["ok"] is True
            assert desk.structuredContent["app"] == "desktop"
            assert desk.structuredContent["marks_count"] == 3

            drag_req = await session.call_tool(
                "desktop",
                {"action": "drag", "arguments": {"from_mark": "m1", "to_mark": "m2"}},
            )
            assert drag_req.structuredContent["code"] == "confirmation_required"
            drag_done = await session.call_tool(
                "computer", {"action": "confirm", "arguments": {"token": drag_req.structuredContent["token"]}}
            )
            assert drag_done.structuredContent["ok"] is True

            await session.call_tool("visual_inspect", {"app": "Calculator"})
            type_req = await session.call_tool(
                "computer",
                {"action": "type", "arguments": {"mark": "m2", "text": "real input"}},
            )
            assert type_req.structuredContent["code"] == "confirmation_required"
            type_done = await session.call_tool(
                "computer", {"action": "confirm", "arguments": {"token": type_req.structuredContent["token"]}}
            )
            assert type_done.structuredContent["ok"] is True


@pytest.mark.anyio
@pytest.mark.parametrize("policy_name", ["policy.autonomous.yaml", "policy.yaml"])
async def test_rust_mcp_direct_policy_executes_without_confirmation_loop(tmp_path, policy_name):
    if not RUST_BIN.is_file():
        pytest.skip("Rust release binary not built")
    env = dict(
        os.environ,
        HANDSMCP_STATE_DIR=str(tmp_path / f"state_{policy_name}"),
        HANDSMCP_POLICY=str(ROOT / policy_name),
    )
    params = StdioServerParameters(command=str(RUST_BIN), args=["--mock"], env=env)
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            vis = await session.call_tool("visual_inspect", {"app": "Calculator"})
            assert vis.structuredContent["ok"] is True

            clk = await session.call_tool("desktop", {"action": "visual_click", "arguments": {"mark": "m2"}})
            assert clk.structuredContent["ok"] is True
            assert clk.structuredContent["policy"] == "direct"
            assert clk.structuredContent["method"] == "mock_ok"

            await session.call_tool("visual_inspect", {"app": "Calculator"})
            typ = await session.call_tool("computer", {"action": "type", "arguments": {"mark": "m2", "text": "hello"}})
            assert typ.structuredContent["ok"] is True
            assert typ.structuredContent["policy"] == "direct"

            # Fast OCR-off + max_marks SoM mode
            vis_fast = await session.call_tool(
                "visual_inspect", {"app": "Calculator", "ocr": "off", "max_marks": 1}
            )
            assert vis_fast.structuredContent["ok"] is True
            assert vis_fast.structuredContent["marks_count"] == 1

            # Direct open_url in desktop tool
            url_res = await session.call_tool(
                "desktop",
                {
                    "action": "open_url",
                    "arguments": {"app": "com.google.Chrome", "url": "https://example.com"},
                },
            )
            assert url_res.structuredContent["ok"] is True
            assert url_res.structuredContent["url"] == "https://example.com"

            # Chained run_actions batch execution + local wait_for
            await session.call_tool("inspect_app", {"app": "Calculator"})
            batch = await session.call_tool(
                "computer",
                {
                    "action": "run_actions",
                    "arguments": {
                        "inspect_after": True,
                        "steps": [
                            {"action": "click", "ref": "e1"},
                            {"action": "type", "ref": "e2", "text": "batch e2e"},
                            {"action": "press_key", "key": "enter"},
                        ],
                    },
                },
            )
            assert batch.structuredContent["ok"] is True
            assert batch.structuredContent["executed"] == 3
            assert "content" in batch.structuredContent

            wait = await session.call_tool(
                "computer",
                {
                    "action": "wait_for",
                    "arguments": {
                        "app": "Calculator",
                        "window": 0,
                        "query": "Notes",
                        "timeout_ms": 500,
                    },
                },
            )
            assert wait.structuredContent["ok"] is True
            assert wait.structuredContent["source"] == "ax"

            await session.call_tool("inspect_app", {"app": "Calculator"})
            coord_click = await session.call_tool(
                "computer",
                {
                    "action": "click",
                    "arguments": {"x": 145, "y": 240},
                },
            )
            assert coord_click.structuredContent["ok"] is True

            await session.call_tool("inspect_app", {"app": "Calculator"})
            targeted_scroll = await session.call_tool(
                "computer",
                {
                    "action": "scroll",
                    "arguments": {
                        "direction": "down",
                        "amount": 10,
                        "ref": "e2",
                        "verify_change": True,
                        "max_retries": 1,
                        "settle_ms": 10,
                        "retry_delay_ms": 10,
                    },
                },
            )
            assert targeted_scroll.structuredContent["ok"] is True
            assert targeted_scroll.structuredContent["attempts"] == 2
            assert targeted_scroll.structuredContent["retried"] == 1
            assert targeted_scroll.structuredContent["verified_change"] is False

            # Executable workflow_memory replay with step_args
            rec = await session.call_tool(
                "workflow_memory",
                {
                    "action": "record",
                    "app": "Calculator",
                    "goal": "Fast note entry",
                    "steps": [
                        {"action": "click", "target_role": "Button", "target_label": "7"},
                        {"action": "type", "target_role": "TextArea", "target_label": "Notes"},
                    ],
                },
            )
            assert rec.structuredContent["ok"] is True
            rep = await session.call_tool(
                "workflow_memory",
                {
                    "action": "replay",
                    "app": "Calculator",
                    "goal": "Fast note entry",
                    "step_args": [None, {"text": "replayed via mcp"}],
                },
            )
            assert rep.structuredContent["ok"] is True
            assert rep.structuredContent["completed_steps"] == 2

