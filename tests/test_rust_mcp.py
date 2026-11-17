import base64
import json
import os
import subprocess
import sys
from pathlib import Path
import pytest
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[1]
RUST_BIN = ROOT / "rust/target/release" / ("apexuse-mcp.exe" if sys.platform == "win32" else "apexuse-mcp")


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _find_image_content(content):
    return next(c for c in content if c.type == "image")


@pytest.mark.anyio
async def test_rust_mcp_stdio_end_to_end(tmp_path):
    if not RUST_BIN.is_file():
        pytest.skip("Rust release binary not built")
    confirm_policy = tmp_path / "confirm_policy.yaml"
    confirm_policy.write_text("mode: normal\nconfirmation: required\nallowed_apps: [\"*\"]\n")
    env = dict(os.environ, APEXUSE_STATE_DIR=str(tmp_path / "state"), APEXUSE_POLICY=str(confirm_policy))
    params = StdioServerParameters(command=str(RUST_BIN), args=["--mock"], env=env)
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            init = await session.initialize()
            assert init.serverInfo.name == "apexuse-mcp"
            assert init.serverInfo.version == "0.3.0"
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
            assert insp.structuredContent["state_mode"] == "full"
            assert "@(" not in insp.structuredContent["content"]
            assert len(insp.structuredContent["workflow_hints"]) == 1

            insp_diff = await session.call_tool("inspect_app", {"app": "Calculator"})
            assert insp_diff.structuredContent["ok"] is True
            assert insp_diff.structuredContent["state_mode"] == "unchanged"

            insp_bounds = await session.call_tool(
                "inspect_app",
                {"app": "Calculator", "diff": False, "include_bounds": True},
            )
            assert insp_bounds.structuredContent["ok"] is True
            assert insp_bounds.structuredContent["state_mode"] == "full"
            assert "@(" in insp_bounds.structuredContent["content"]

            # 3. Screenshot defaults to JPEG and supports explicit PNG
            ss_def = await session.call_tool("screenshot", {"app": "Calculator"})
            assert ss_def.structuredContent["ok"] is True
            assert ss_def.structuredContent["image_format"] == "jpeg"
            ss_def_img = _find_image_content(ss_def.content)
            assert ss_def_img.mimeType == "image/jpeg"
            assert base64.b64decode(ss_def_img.data).startswith(b"\xff\xd8\xff")

            ss_png = await session.call_tool("screenshot", {"app": "Calculator", "format": "png"})
            assert ss_png.structuredContent["ok"] is True
            assert ss_png.structuredContent["image_format"] == "png"
            ss_png_img = _find_image_content(ss_png.content)
            assert ss_png_img.mimeType == "image/png"
            assert base64.b64decode(ss_png_img.data).startswith(b"\x89PNG\r\n\x1a\n")

            # 4. Set-of-Mark visual_inspect defaults to JPEG, supports explicit PNG, and fuses mark index
            vis = await session.call_tool("visual_inspect", {"app": "Calculator"})
            assert vis.structuredContent["ok"] is True
            assert vis.structuredContent["marks_count"] == 3
            assert vis.structuredContent["image_format"] == "jpeg"
            assert vis.structuredContent["image_bytes"] > 0
            vis_img = _find_image_content(vis.content)
            assert vis_img.mimeType == "image/jpeg"
            assert base64.b64decode(vis_img.data).startswith(b"\xff\xd8\xff")

            vis_png = await session.call_tool("visual_inspect", {"app": "Calculator", "format": "png"})
            assert vis_png.structuredContent["ok"] is True
            assert vis_png.structuredContent["image_format"] == "png"
            vis_png_img = _find_image_content(vis_png.content)
            assert vis_png_img.mimeType == "image/png"
            assert base64.b64decode(vis_png_img.data).startswith(b"\x89PNG\r\n\x1a\n")

            # 5. Visual click by Set-of-Mark ID requires confirmation, then executes on confirm
            clk = await session.call_tool("desktop", {"action": "visual_click", "arguments": {"mark": "m2"}})
            assert clk.structuredContent["code"] == "confirmation_required"
            tok = clk.structuredContent["token"]

            done = await session.call_tool("computer", {"action": "confirm", "arguments": {"token": tok}})
            assert done.structuredContent["ok"] is True
            assert done.structuredContent["method"] == "mock_ok"

            # 6. Desktop-wide Set-of-Mark visual_inspect + drag + type by mark
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
    state_key = "APEXUSE_STATE_DIR" if policy_name == "policy.autonomous.yaml" else "HANDSMCP_STATE_DIR"
    policy_key = "APEXUSE_POLICY" if policy_name == "policy.autonomous.yaml" else "HANDSMCP_POLICY"
    env = dict(os.environ)
    env[state_key] = str(tmp_path / f"state_{policy_name}")
    env[policy_key] = str(ROOT / policy_name)
    params = StdioServerParameters(command=str(RUST_BIN), args=["--mock"], env=env)
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            vis = await session.call_tool("visual_inspect", {"app": "Calculator"})
            assert vis.structuredContent["ok"] is True
            assert vis.structuredContent["image_format"] == "jpeg"

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

            # Native set_value (value set + "" clear), select_text, fail-closed (x,y), and keep_snapshot form batching
            await session.call_tool("inspect_app", {"app": "Calculator"})
            sv_set = await session.call_tool(
                "computer",
                {"action": "set_value", "arguments": {"ref": "e2", "text": "native value"}},
            )
            assert sv_set.structuredContent["ok"] is True
            assert sv_set.structuredContent["snapshot_preserved"] is True

            # Form ref e2 survives because reuse_form_refs defaults to true
            sel = await session.call_tool(
                "computer",
                {"action": "select_text", "arguments": {"ref": "e2", "start": 0, "end": 6}},
            )
            assert sel.structuredContent["ok"] is True
            assert sel.structuredContent["snapshot_preserved"] is True

            # Empty text "" clears cleanly via set_value
            sv_clear = await session.call_tool(
                "computer",
                {"action": "set_value", "arguments": {"ref": "e2", "text": ""}},
            )
            assert sv_clear.structuredContent["ok"] is True
            assert sv_clear.structuredContent["snapshot_preserved"] is True

            # Semantic target works without explicit ref and respects keep_snapshot=false
            sem_set = await session.call_tool(
                "computer",
                {
                    "action": "set_value",
                    "arguments": {
                        "target": {"query": "Notes", "role": "TextArea"},
                        "text": "semantic draft",
                        "keep_snapshot": False,
                    },
                },
            )
            assert sem_set.structuredContent["ok"] is True
            assert sem_set.structuredContent["snapshot_preserved"] is False

            # Prior ref e2 is now invalidated after keep_snapshot=false
            stale = await session.call_tool(
                "computer",
                {"action": "set_value", "arguments": {"ref": "e2", "text": "should fail"}},
            )
            assert stale.structuredContent["ok"] is False
            assert stale.structuredContent["code"] == "stale_ref"

            # Re-inspect and verify fail-closed target_identity_unverified on raw (x, y)
            await session.call_tool("inspect_app", {"app": "Calculator"})
            bad_xy = await session.call_tool(
                "computer",
                {"action": "set_value", "arguments": {"x": 145, "y": 240, "text": "unsafe"}},
            )
            assert bad_xy.structuredContent["ok"] is False
            assert bad_xy.structuredContent["code"] == "target_identity_unverified"

            bad_sel_xy = await session.call_tool(
                "computer",
                {"action": "select_text", "arguments": {"x": 145, "y": 240}},
            )
            assert bad_sel_xy.structuredContent["ok"] is False
            assert bad_sel_xy.structuredContent["code"] == "target_identity_unverified"

            # Semantic target on Button click
            sem_clk = await session.call_tool(
                "computer",
                {"action": "click", "arguments": {"target": {"query": "7", "role": "Button"}}},
            )
            assert sem_clk.structuredContent["ok"] is True

            # Chained run_actions batch execution (with set_value, select_text, target) + local wait_for
            await session.call_tool("inspect_app", {"app": "Calculator"})
            batch = await session.call_tool(
                "computer",
                {
                    "action": "run_actions",
                    "arguments": {
                        "inspect_after": True,
                        "steps": [
                            {"action": "set_value", "ref": "e2", "text": "batch e2e"},
                            {"action": "select_text", "ref": "e2", "start": 0, "end": 5},
                            {"action": "click", "target": {"query": "7", "role": "Button"}},
                            {"action": "press_key", "key": "enter"},
                        ],
                    },
                },
            )
            assert batch.structuredContent["ok"] is True
            assert batch.structuredContent["executed"] == 4
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

            # AX-first diffing, inline return_state="diff" keeping refs live, and verify_mode="ax"
            insp_base = await session.call_tool("inspect_app", {"app": "Calculator", "diff": False})
            assert insp_base.structuredContent["ok"] is True
            assert insp_base.structuredContent["state_mode"] == "full"
            assert "@(" not in insp_base.structuredContent["content"]

            clk_diff = await session.call_tool(
                "computer",
                {"action": "click", "arguments": {"ref": "e1", "return_state": "diff"}},
            )
            assert clk_diff.structuredContent["ok"] is True
            assert "content" in clk_diff.structuredContent
            assert clk_diff.structuredContent["state_mode"] == "unchanged"
            assert clk_diff.structuredContent["snapshot_active"] is True
            assert clk_diff.structuredContent["snapshot_preserved"] is True

            # Ref e2 and e1 remain immediately usable without an extra inspect_app call
            sv_live = await session.call_tool(
                "computer",
                {"action": "set_value", "arguments": {"ref": "e2", "text": "post return_state"}},
            )
            assert sv_live.structuredContent["ok"] is True
            assert sv_live.structuredContent["snapshot_preserved"] is True

            clk_ax = await session.call_tool(
                "computer",
                {
                    "action": "click",
                    "arguments": {
                        "ref": "e1",
                        "verify_change": True,
                        "verify_mode": "ax",
                        "max_retries": 0,
                        "settle_ms": 10,
                    },
                },
            )
            assert clk_ax.structuredContent["ok"] is True
            assert clk_ax.structuredContent["attempts"] == 1
            assert clk_ax.structuredContent["verified_change"] is False
            assert clk_ax.structuredContent["ax_changed"] is False
            assert clk_ax.structuredContent["visual_changed"] is False


def test_rust_cli_subcommands_end_to_end(tmp_path):
    if not RUST_BIN.is_file():
        pytest.skip("Rust release binary not built")

    fake_home = tmp_path / "home"
    fake_home.mkdir()
    appdata = tmp_path / "appdata"
    appdata.mkdir()
    xdg_config = fake_home / ".config"
    xdg_config.mkdir()
    state_dir = tmp_path / "state"
    env = dict(
        os.environ,
        HOME=str(fake_home),
        USERPROFILE=str(fake_home),
        APPDATA=str(appdata),
        XDG_CONFIG_HOME=str(xdg_config),
        APEXUSE_STATE_DIR=str(state_dir),
        APEXUSE_POLICY=str(ROOT / "policy.autonomous.yaml"),
    )
    env.pop("APEXUSE_CONFIG", None)

    # 1. --version / -V
    ver = subprocess.run([str(RUST_BIN), "--version"], capture_output=True, text=True, env=env)
    assert ver.returncode == 0
    assert ver.stdout.strip() == "apexuse-mcp 0.3.0"

    # 2. config set / get / show / reset (plus APEXUSE_CONFIG override)
    set_fmt = subprocess.run(
        [str(RUST_BIN), "config", "set", "image.format", "png"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert set_fmt.returncode == 0, set_fmt.stderr
    assert json.loads(set_fmt.stdout)["value"] == "png"

    get_fmt = subprocess.run(
        [str(RUST_BIN), "config", "get", "image.format"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert get_fmt.returncode == 0
    assert json.loads(get_fmt.stdout)["value"] == "png"

    set_ax_diff = subprocess.run(
        [str(RUST_BIN), "config", "set", "snapshot.ax_diff", "false"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert set_ax_diff.returncode == 0, set_ax_diff.stderr
    assert json.loads(set_ax_diff.stdout)["value"] is False

    get_ax_diff = subprocess.run(
        [str(RUST_BIN), "config", "get", "snapshot.ax_diff"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert get_ax_diff.returncode == 0
    assert json.loads(get_ax_diff.stdout)["value"] is False

    set_bounds = subprocess.run(
        [str(RUST_BIN), "config", "set", "snapshot.include_bounds", "true"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert set_bounds.returncode == 0, set_bounds.stderr
    assert json.loads(set_bounds.stdout)["value"] is True

    get_bounds = subprocess.run(
        [str(RUST_BIN), "config", "get", "snapshot.include_bounds"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert get_bounds.returncode == 0
    assert json.loads(get_bounds.stdout)["value"] is True

    show_cfg = subprocess.run([str(RUST_BIN), "config", "show"], capture_output=True, text=True, env=env)
    assert show_cfg.returncode == 0
    show_json = json.loads(show_cfg.stdout)["config"]
    assert show_json["image"]["format"] == "png"
    assert show_json["snapshot"]["ax_diff"] is False
    assert show_json["snapshot"]["include_bounds"] is True

    reset_cfg = subprocess.run([str(RUST_BIN), "config", "reset"], capture_output=True, text=True, env=env)
    assert reset_cfg.returncode == 0
    reset_json = json.loads(reset_cfg.stdout)["config"]
    assert reset_json["image"]["format"] == "jpeg"
    assert reset_json["snapshot"]["ax_diff"] is True
    assert reset_json["snapshot"]["include_bounds"] is False

    custom_cfg_path = tmp_path / "custom_config.json"
    env_custom = dict(env, APEXUSE_CONFIG=str(custom_cfg_path))
    set_q = subprocess.run(
        [str(RUST_BIN), "config", "set", "image.quality", "65"],
        capture_output=True,
        text=True,
        env=env_custom,
    )
    assert set_q.returncode == 0
    assert custom_cfg_path.is_file()
    assert json.loads(custom_cfg_path.read_text())["image"]["quality"] == 65

    # 3. call --mock --calls '...' shares single Gateway snapshot refs across steps
    calls_json = json.dumps(
        [
            {"tool": "inspect_app", "args": {"app": "Calculator"}},
            {"tool": "computer", "args": {"action": "set_value", "arguments": {"ref": "e2", "text": "cli batch"}}},
            {"tool": "computer", "args": {"action": "select_text", "arguments": {"ref": "e2", "start": 0, "end": 3}}},
        ]
    )
    seq = subprocess.run(
        [str(RUST_BIN), "call", "--mock", "--calls", calls_json, "--sleep-ms", "1"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert seq.returncode == 0, seq.stderr
    seq_data = json.loads(seq.stdout)
    assert seq_data["ok"] is True
    assert seq_data["completed"] == 3
    assert seq_data["results"][1]["result"]["snapshot_preserved"] is True

    # Single call --save-image writes valid JPEG bytes
    img_out = tmp_path / "out.jpg"
    shot = subprocess.run(
        [
            str(RUST_BIN),
            "call",
            "--mock",
            "screenshot",
            "--args",
            '{"app":"Calculator"}',
            "--save-image",
            str(img_out),
        ],
        capture_output=True,
        text=True,
        env=env,
    )
    assert shot.returncode == 0, shot.stderr
    assert img_out.read_bytes().startswith(b"\xff\xd8\xff")

    # Verify config.json wires into Gateway::with_config for CLI `call`
    subprocess.run([str(RUST_BIN), "config", "set", "image.format", "png"], check=True, env=env)
    png_out = tmp_path / "out.png"
    calls_file = tmp_path / "calls.json"
    calls_file.write_text(json.dumps([{"tool": "visual_inspect", "args": {"app": "Calculator"}}]))
    seq_file = subprocess.run(
        [str(RUST_BIN), "call", "--mock", "--calls-file", str(calls_file), "--save-image", str(png_out)],
        capture_output=True,
        text=True,
        env=env,
    )
    assert seq_file.returncode == 0, seq_file.stderr
    assert json.loads(seq_file.stdout)["results"][0]["result"]["image_format"] == "png"
    assert png_out.read_bytes().startswith(b"\x89PNG\r\n\x1a\n")
    subprocess.run([str(RUST_BIN), "config", "reset"], check=True, env=env)

    # Verify snapshot.ax_diff and snapshot.include_bounds wire into Gateway::with_config for CLI `call`
    subprocess.run([str(RUST_BIN), "config", "set", "snapshot.ax_diff", "false"], check=True, env=env)
    subprocess.run([str(RUST_BIN), "config", "set", "snapshot.include_bounds", "true"], check=True, env=env)
    ax_cfg_calls = json.dumps(
        [
            {"tool": "inspect_app", "args": {"app": "Calculator"}},
            {"tool": "inspect_app", "args": {"app": "Calculator"}},
        ]
    )
    ax_cfg_res = subprocess.run(
        [str(RUST_BIN), "call", "--mock", "--calls", ax_cfg_calls],
        capture_output=True,
        text=True,
        env=env,
    )
    assert ax_cfg_res.returncode == 0, ax_cfg_res.stderr
    ax_cfg_data = json.loads(ax_cfg_res.stdout)
    assert ax_cfg_data["results"][0]["result"]["state_mode"] == "full"
    assert "@(" in ax_cfg_data["results"][0]["result"]["content"]
    assert ax_cfg_data["results"][1]["result"]["state_mode"] == "full"
    subprocess.run([str(RUST_BIN), "config", "reset"], check=True, env=env)

    # Error exit codes on invalid config key and failing tool call
    bad_cfg = subprocess.run([str(RUST_BIN), "config", "set", "bad.key", "1"], capture_output=True, text=True, env=env)
    assert bad_cfg.returncode == 1
    bad_call = subprocess.run([str(RUST_BIN), "call", "--mock", "inspect_app", "--args", "{}"], capture_output=True, text=True, env=env)
    assert bad_call.returncode == 1

    # 4. install cursor --dry-run --npx, install all without flags fails, install cursor --write --with-skill
    inst = subprocess.run(
        [str(RUST_BIN), "install", "cursor", "--dry-run", "--npx"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert inst.returncode == 0, inst.stderr
    inst_data = json.loads(inst.stdout)
    assert inst_data["ok"] is True
    assert inst_data["agent"] == "cursor"
    assert inst_data["mode"] == "dry_run"
    cursor_entry = inst_data["config"]["mcpServers"]["apexuse"]
    assert cursor_entry["command"] == "npx"
    assert cursor_entry["args"] == ["-y", "apexuse-mcp"]
    assert not (fake_home / ".cursor/mcp.json").exists()

    bare_all = subprocess.run([str(RUST_BIN), "install", "all"], capture_output=True, text=True, env=env)
    assert bare_all.returncode == 1

    inst_write = subprocess.run(
        [str(RUST_BIN), "install", "cursor", "--write", "--with-skill"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert inst_write.returncode == 0, inst_write.stderr
    assert (fake_home / ".cursor/mcp.json").is_file()
    skill_text = (ROOT / ".agents/skills/apexuse/SKILL.md").read_text(encoding="utf-8")
    assert (ROOT / "rust/assets/SKILL.md").read_text(encoding="utf-8") == skill_text
    assert (fake_home / ".agents/skills/apexuse/SKILL.md").read_text(encoding="utf-8") == skill_text

    # 5. doctor --check (exits 0/1 consistently with JSON flags; --mock --check exits 0)
    doc = subprocess.run([str(RUST_BIN), "doctor", "--check"], capture_output=True, text=True, env=env)
    doc_data = json.loads(doc.stdout)
    expected_code = 0 if (doc_data.get("accessibility") and doc_data.get("screen_recording")) else 1
    assert doc.returncode == expected_code

    doc_mock = subprocess.run([str(RUST_BIN), "doctor", "--mock", "--check"], capture_output=True, text=True, env=env)
    assert doc_mock.returncode == 0
    assert json.loads(doc_mock.stdout)["accessibility"] is True


def test_plugin_and_skill_assets_in_sync():
    asset_pairs = [
        (ROOT / "rust/assets/SKILL.md", ROOT / ".agents/skills/apexuse/SKILL.md"),
        (ROOT / "rust/assets/plugin/marketplace.json", ROOT / ".agents/plugins/marketplace.json"),
        (ROOT / "rust/assets/plugin/plugin.json", ROOT / "plugins/apexuse/.codex-plugin/plugin.json"),
        (ROOT / "rust/assets/plugin/apexuse.svg", ROOT / "plugins/apexuse/assets/apexuse.svg"),
        (ROOT / "rust/assets/plugin/apexuse-small.svg", ROOT / "plugins/apexuse/assets/apexuse-small.svg"),
    ]
    for crate_asset, repo_asset in asset_pairs:
        assert crate_asset.is_file(), f"Missing crate asset: {crate_asset}"
        assert repo_asset.is_file(), f"Missing repo asset: {repo_asset}"
        assert crate_asset.read_bytes() == repo_asset.read_bytes(), (
            f"Asset mismatch between {crate_asset} and {repo_asset}"
        )

    skill_dir = ROOT / ".agents/skills/apexuse"
    openai_yaml = skill_dir / "agents/openai.yaml"
    assert openai_yaml.is_file(), f"Missing {openai_yaml}"
    yaml_text = openai_yaml.read_text(encoding="utf-8")
    assert 'display_name: "ApexUse"' in yaml_text
    assert "short_description:" in yaml_text
    assert "default_prompt:" in yaml_text

    for ref_name in ["installation.md", "usage.md", "troubleshooting.md"]:
        ref_path = skill_dir / "references" / ref_name
        assert ref_path.is_file(), f"Missing skill reference doc: {ref_path}"
        assert len(ref_path.read_text(encoding="utf-8").strip()) > 0


def test_rust_cli_codex_plugin_probe_and_onboard_end_to_end(tmp_path):
    if not RUST_BIN.is_file():
        pytest.skip("Rust release binary not built")

    fake_home = tmp_path / "home"
    fake_home.mkdir()
    appdata = tmp_path / "appdata"
    appdata.mkdir()
    xdg_config = fake_home / ".config"
    xdg_config.mkdir()
    state_dir = tmp_path / "state"
    env = dict(
        os.environ,
        HOME=str(fake_home),
        USERPROFILE=str(fake_home),
        APPDATA=str(appdata),
        XDG_CONFIG_HOME=str(xdg_config),
        APEXUSE_STATE_DIR=str(state_dir),
        APEXUSE_POLICY=str(ROOT / "policy.autonomous.yaml"),
    )
    env.pop("APEXUSE_CONFIG", None)

    # 1. Standalone probe --mock
    probe_res = subprocess.run([str(RUST_BIN), "probe", "--mock"], capture_output=True, text=True, env=env)
    assert probe_res.returncode == 0, probe_res.stderr
    probe_data = json.loads(probe_res.stdout)
    assert probe_data["ok"] is True
    assert probe_data["server_name"] == "apexuse-mcp"
    assert probe_data["tools_count"] == 6
    assert set(probe_data["tools"]) == {
        "inspect_app",
        "visual_inspect",
        "computer",
        "desktop",
        "screenshot",
        "workflow_memory",
    }

    # 2. install codex-plugin --dry-run
    dry_res = subprocess.run(
        [str(RUST_BIN), "install", "codex-plugin", "--dry-run"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert dry_res.returncode == 0, dry_res.stderr
    dry_data = json.loads(dry_res.stdout)
    assert dry_data["ok"] is True
    assert dry_data["agent"] == "codex-plugin"
    assert dry_data["mode"] == "dry_run"
    assert dry_data["status"] == "would_write"
    assert not (fake_home / ".codex/plugins").exists()
    assert not (fake_home / ".codex/config.toml").exists()

    # 3. install codex-plugin --write --probe --mock
    write_res = subprocess.run(
        [str(RUST_BIN), "install", "codex-plugin", "--write", "--probe", "--mock"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert write_res.returncode == 0, write_res.stderr
    write_data = json.loads(write_res.stdout)
    assert write_data["ok"] is True
    assert write_data["agent"] == "codex-plugin"
    assert write_data["status"] == "written"
    assert write_data["probe"]["ok"] is True
    assert write_data["probe"]["tools_count"] == 6

    marketplace_dir = fake_home / ".codex/plugins/marketplaces/apexuse-local"
    cache_dir = fake_home / ".codex/plugins/cache/apexuse-local/apexuse/0.3.0"
    assert (marketplace_dir / ".agents/plugins/marketplace.json").read_bytes() == (
        ROOT / "rust/assets/plugin/marketplace.json"
    ).read_bytes()
    assert (cache_dir / ".codex-plugin/plugin.json").read_bytes() == (
        ROOT / "rust/assets/plugin/plugin.json"
    ).read_bytes()
    assert (cache_dir / "assets/apexuse.svg").read_bytes() == (
        ROOT / "rust/assets/plugin/apexuse.svg"
    ).read_bytes()
    assert (cache_dir / "assets/apexuse-small.svg").read_bytes() == (
        ROOT / "rust/assets/plugin/apexuse-small.svg"
    ).read_bytes()

    for mcp_json_path in [marketplace_dir / "plugins/apexuse/.mcp.json", cache_dir / ".mcp.json"]:
        assert mcp_json_path.is_file()
        mcp_text = mcp_json_path.read_text(encoding="utf-8")
        assert "--mock" not in mcp_text
        mcp_json = json.loads(mcp_text)
        entry = mcp_json["mcpServers"]["apexuse"]
        assert "--mock" not in entry["args"]

    codex_toml = fake_home / ".codex/config.toml"
    toml_text = codex_toml.read_text(encoding="utf-8")
    assert "[marketplaces.apexuse-local]" in toml_text
    assert '[plugins."apexuse@apexuse-local"]' in toml_text
    assert "enabled = true" in toml_text
    assert "[mcp_servers.apexuse]" not in toml_text

    # 4. install all --write preserves active codex-plugin
    all_res = subprocess.run(
        [str(RUST_BIN), "install", "all", "--write"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert all_res.returncode == 0, all_res.stderr
    all_data = json.loads(all_res.stdout)
    wired_agents = [item["agent"] for item in all_data["wired"]]
    assert "codex-plugin" in wired_agents
    assert "codex" not in wired_agents
    toml_after_all = codex_toml.read_text(encoding="utf-8")
    assert "[mcp_servers.apexuse]" not in toml_after_all
    assert '[plugins."apexuse@apexuse-local"]' in toml_after_all
    assert "enabled = true" in toml_after_all

    # 5. install codex --write disables codex-plugin and writes [mcp_servers.apexuse]
    codex_res = subprocess.run(
        [str(RUST_BIN), "install", "codex", "--write"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert codex_res.returncode == 0, codex_res.stderr
    toml_after_codex = codex_toml.read_text(encoding="utf-8")
    assert "[mcp_servers.apexuse]" in toml_after_codex
    assert '[plugins."apexuse@apexuse-local"]' in toml_after_codex
    assert "enabled = false" in toml_after_codex
    assert "enabled = true" not in toml_after_codex

    # 6. onboard --mock --check and doctor --onboard --mock --check exit 0 with shown: false
    onboard_res = subprocess.run(
        [str(RUST_BIN), "onboard", "--mock", "--check"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert onboard_res.returncode == 0, onboard_res.stderr
    onboard_data = json.loads(onboard_res.stdout)
    assert onboard_data["ok"] is True
    assert onboard_data["shown"] is False

    doc_onboard_res = subprocess.run(
        [str(RUST_BIN), "doctor", "--onboard", "--mock", "--check"],
        capture_output=True,
        text=True,
        env=env,
    )
    assert doc_onboard_res.returncode == 0, doc_onboard_res.stderr
    doc_onboard_data = json.loads(doc_onboard_res.stdout)
    assert doc_onboard_data["ok"] is True
    assert doc_onboard_data["shown"] is False


@pytest.mark.anyio
async def test_rust_mcp_domain_policy_partial_batch_and_replay_preconditions_e2e(tmp_path):
    if not RUST_BIN.is_file():
        pytest.skip("Rust release binary not built")

    dom_policy = tmp_path / "domain_policy.yaml"
    dom_policy.write_text(
        'mode: normal\nconfirmation: direct\nconfirm_destructive: true\n'
        'allowed_apps: ["*"]\n'
        'allowed_domains: ["example.com", "*.docs.org"]\n'
        'blocked_domains: ["evil.docs.org"]\n',
        encoding="utf-8",
    )
    env = dict(
        os.environ,
        APEXUSE_STATE_DIR=str(tmp_path / "state"),
        APEXUSE_POLICY=str(dom_policy),
    )
    env.pop("APEXUSE_CONFIG", None)
    params = StdioServerParameters(command=str(RUST_BIN), args=["--mock"], env=env)
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()

            # 1. Domain policy enforces blocked_domains & allowed_domains on open_url
            bad_url = await session.call_tool(
                "desktop",
                {
                    "action": "open_url",
                    "arguments": {"app": "com.google.Chrome", "url": "https://evil.docs.org/phish"},
                },
            )
            assert bad_url.structuredContent["ok"] is False
            assert bad_url.structuredContent["code"] == "domain_blocked"

            ok_url = await session.call_tool(
                "desktop",
                {
                    "action": "open_url",
                    "arguments": {"app": "com.google.Chrome", "url": "https://guide.docs.org/"},
                },
            )
            assert ok_url.structuredContent["ok"] is True

            # 2. run_actions reports failed_step, executed count, and completed results on mid-batch failure
            insp = await session.call_tool("inspect_app", {"app": "Calculator"})
            assert insp.structuredContent["ok"] is True

            partial = await session.call_tool(
                "computer",
                {
                    "action": "run_actions",
                    "arguments": {
                        "steps": [
                            {"action": "press_key", "key": "return"},
                            {"action": "click", "ref": "e999"},
                        ]
                    },
                },
            )
            assert partial.structuredContent["ok"] is False
            assert partial.structuredContent["code"] == "stale_ref"
            assert partial.structuredContent["failed_step"] == 1
            assert partial.structuredContent["executed"] == 1
            assert len(partial.structuredContent["results"]) == 1
            assert partial.structuredContent["results"][0]["step"] == 0

            # 3. workflow_memory rejects replay when expected_window_contains precondition mismatches
            rec = await session.call_tool(
                "workflow_memory",
                {
                    "action": "record",
                    "app": "Calculator",
                    "goal": "Strict window replay",
                    "expected_window_contains": "NonexistentModalTitle",
                    "steps": [{"action": "click", "target_role": "Button", "target_label": "7", "note": "Press 7"}],
                },
            )
            assert rec.structuredContent["ok"] is True

            pre_fail = await session.call_tool(
                "workflow_memory",
                {"action": "replay", "app": "Calculator", "goal": "Strict window"},
            )
            assert pre_fail.structuredContent["ok"] is False
            assert pre_fail.structuredContent["code"] == "precondition_failed"
