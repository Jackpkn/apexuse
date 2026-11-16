import json
import tomllib
from handsmcp.cli import config_for

def test_antigravity_config_has_absolute_paths(tmp_path):
    config = json.loads(config_for('antigravity', tmp_path))
    server = config['mcpServers']['handsmcp']
    assert server['command'] == str(tmp_path / '.venv/bin/python')
    assert server['args'] == ['-m','handsmcp.server']
    assert server['env']['PYTHONPATH'] == str(tmp_path / 'src')

def test_codex_config_is_valid_toml(tmp_path):
    config = tomllib.loads(config_for('codex', tmp_path))
    assert config['mcp_servers']['handsmcp']['command'] == str(tmp_path / '.venv/bin/python')

def test_gemini_and_claude_are_distinct_formats(tmp_path):
    assert 'handsmcp' in json.loads(config_for('gemini',tmp_path))['mcpServers']
    assert config_for('claude',tmp_path).startswith('claude mcp add ')

def test_cua_preview_config_is_portable_for_all_agents(tmp_path):
    for agent in ('antigravity','gemini'):
        server=json.loads(config_for(agent,tmp_path,backend='cua'))['mcpServers']['handsmcp']
        assert server['env']['HANDSMCP_BACKEND'] == 'cua'
    cfg=tomllib.loads(config_for('codex',tmp_path,backend='cua'))
    assert cfg['mcp_servers']['handsmcp']['env']['HANDSMCP_BACKEND'] == 'cua'
    assert 'HANDSMCP_BACKEND=cua' in config_for('claude',tmp_path,backend='cua')

def test_invalid_backend_config_rejected(tmp_path):
    import pytest
    with pytest.raises(ValueError): config_for('codex',tmp_path,backend='typo')


def test_rust_backend_config_points_to_compiled_binary(tmp_path):
    expected_bin = str(tmp_path / 'rust/target/release/handsmcp-mcp')
    for agent in ('antigravity', 'gemini'):
        server = json.loads(config_for(agent, tmp_path, backend='rust'))['mcpServers']['handsmcp']
        assert server['command'] == expected_bin
        assert server['args'] == []
    cfg = tomllib.loads(config_for('codex', tmp_path, backend='rust'))
    assert cfg['mcp_servers']['handsmcp']['command'] == expected_bin
    assert config_for('claude', tmp_path, backend='rust').endswith(expected_bin)
