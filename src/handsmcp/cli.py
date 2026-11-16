"""Local server entry point and non-destructive client configuration output."""
import argparse
import json
from pathlib import Path
import shlex


def config_for(agent, root, backend='munim'):
    if backend not in {'munim', 'cua', 'rust'}:
        raise ValueError('unsupported backend')
    root = Path(root).resolve()
    if backend == 'rust':
        rust_bin = str(root / 'rust/target/release/handsmcp-mcp')
        if agent in {'antigravity', 'gemini'}:
            return json.dumps({'mcpServers': {'handsmcp': {'command': rust_bin, 'args': []}}}, indent=2)
        if agent == 'codex':
            return ('[mcp_servers.handsmcp]\ncommand = ' + json.dumps(rust_bin, ensure_ascii=True) +
                    '\nargs = []\nstartup_timeout_sec = 60\ntool_timeout_sec = 180')
        if agent == 'claude':
            return shlex.join(['claude', 'mcp', 'add', 'handsmcp', '--', rust_bin])
        raise ValueError('unsupported agent')
    python = str(root / '.venv/bin/python')
    source = str(root / 'src')
    env = {'PYTHONPATH': source}
    if backend == 'cua':
        env['HANDSMCP_BACKEND'] = 'cua'
    if agent in {'antigravity', 'gemini'}:
        return json.dumps({'mcpServers': {'handsmcp': {'command': python,
            'args': ['-m', 'handsmcp.server'], 'env': env}}}, indent=2)
    if agent == 'codex':
        quote = lambda value: json.dumps(value, ensure_ascii=True)
        return ('[mcp_servers.handsmcp]\ncommand = ' + quote(python) +
                '\nargs = ["-m", "handsmcp.server"]\nstartup_timeout_sec = 60\n'
                'tool_timeout_sec = 180\n[mcp_servers.handsmcp.env]\nPYTHONPATH = ' + quote(source) +
                ('\nHANDSMCP_BACKEND = "cua"' if backend == 'cua' else ''))
    if agent == 'claude':
        return shlex.join(['claude', 'mcp', 'add', 'handsmcp', '--env',
                           f'PYTHONPATH={source}'] + (['--env', 'HANDSMCP_BACKEND=cua'] if backend == 'cua' else []) +
                          ['--', python, '-m', 'handsmcp.server'])
    raise ValueError('unsupported agent')


def main():
    parser = argparse.ArgumentParser(prog='handsmcp')
    sub = parser.add_subparsers(dest='command')
    sub.add_parser('mcp', help='start the stdio server')
    sub.add_parser('doctor', help='check optional Cua Driver permissions without changing them')
    install = sub.add_parser('install', help='print a client configuration; does not write settings')
    install.add_argument('--agent', choices=('antigravity','claude','gemini','codex'), required=True)
    install.add_argument('--backend', choices=('munim','cua','rust'), default='munim', help='munim, cua hybrid, or standalone rust hybrid SoM binary')
    args = parser.parse_args()
    if args.command == 'doctor':
        from .doctor import check_cua
        print(json.dumps({'cua_driver': check_cua(), 'integration': 'experimental_hybrid'}, indent=2))
    elif args.command == 'install':
        print(config_for(args.agent, Path(__file__).resolve().parents[2], args.backend))
    else:
        from .server import main as run_server
        run_server()

if __name__ == '__main__': main()
