#!/usr/bin/env python3
"""Focused native proof that the agents tab follows what CLI agents report.

Deterministic `claude` and `codex` fixtures run in an isolated instance, read
the hooks Neptune injects for them and fire those hooks, or set the terminal
title, on request. The rows of the agents tab are read back through inspection.
Real provider behaviour requires a separate installed-CLI smoke test.
"""
import argparse
import json
import os
from pathlib import Path
import runpy
import shlex
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
H = runpy.run_path(str(ROOT / 'scripts/native-harness.py'))
APP = ROOT / 'target/debug/neptune'
CLIENT = ROOT / 'target/debug/neptune-inspect'
FIXTURE = r'''#!/usr/bin/env python3
import json, os, shlex, subprocess, sys, tomllib, uuid
from pathlib import Path
args = sys.argv[1:]
provider = Path(sys.argv[0]).name
if '--help' in args:
    print('--no-daemon'); sys.exit(0)
if provider == 'claude':
    settings = json.loads(args[args.index('--settings')+1])
    hooks = {event: groups[0]['hooks'][0] for event, groups in settings['hooks'].items()}
else:
    hooks = {}
    for index, arg in enumerate(args[:-1]):
        if arg == '-c' and args[index+1].startswith('hooks.'):
            for event, groups in tomllib.loads(args[index+1])['hooks'].items():
                hooks[event] = groups[0]['hooks'][0]
session = str(uuid.uuid4())
def fire(event, tool=None, **fields):
    payload = {'session_id': session, 'cwd': os.getcwd(), 'hook_event_name': event,
               'transcript_path': '/nonexistent/transcript.jsonl', 'permission_mode': 'default', **fields}
    if event == 'UserPromptSubmit':
        payload['prompt'] = 'PRIVATE PROMPT'
    if tool:
        payload.update(tool_name=tool, tool_input={'command': 'PRIVATE COMMAND'}, tool_use_id='tool-1')
    if event == 'PostToolUse':
        # Real results are large; the hook must still mark the moment.
        payload['tool_response'] = 'PRIVATE OUTPUT ' * (int(fields.pop('size', 0)) // 15 + 1)
    if event not in hooks:
        print('NO HOOK: ' + event, flush=True); return
    result = subprocess.run(hooks[event]['command'], shell=True, input=json.dumps(payload), text=True, capture_output=True)
    # A hook that prints or fails would steer the agent.
    print(f'FIRED {event} exit={result.returncode} out={len(result.stdout)}', flush=True)
fire('SessionStart', source='startup')
with open(os.environ['NEPTUNE_AGENT_TEST_LOG'], 'a') as log:
    log.write(json.dumps({'provider': provider, 'hooks': sorted(hooks), 'timeouts': {e: h.get('timeout') for e, h in hooks.items()}}) + '\n')
print(provider.upper() + ' READY', flush=True)
while True:
    line = sys.stdin.readline()
    if not line: break
    # Keys typed at a request, such as Escape, arrive with the next line.
    line = line.replace('\x1b', '')
    if line.strip() == 'exit': break
    words = line.split()
    if not words: continue
    if words[0] == 'fire':
        fields = dict(word.split('=', 1) for word in words[2:] if '=' in word)
        tool = next((word for word in words[2:] if '=' not in word), None)
        fire(words[1], tool, **fields)
    elif words[0] == 'title':
        sys.stdout.write('\033]0;' + line.split(None, 1)[1].strip() + '\007'); sys.stdout.flush()
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts')
    options = parser.parse_args()
    options.output.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='agent-activity-', dir=options.output)).resolve()
    data = output / 'data'; data.mkdir()
    home = output / 'home'; home.mkdir()
    fixtures = output / 'bin'; fixtures.mkdir()
    for provider in ('codex', 'claude'):
        path = fixtures/provider; path.write_text(FIXTURE); path.chmod(0o700)
    (data/'config.toml').write_text('shell = "/bin/bash"\nconfirm_close = false\nwarn_running_processes = false\n')
    (home/'.bashrc').write_text(f'export PATH={shlex.quote(str(fixtures))}:"$PATH"\nPS1="test $ "\n')
    endpoint = H['free_endpoint']()
    # A run started from a Neptune terminal must not reach that instance's bridge.
    env = {key: value for key, value in os.environ.items() if not key.startswith('NEPTUNE_AGENT_')}
    env.pop('WAYLAND_DISPLAY', None)
    path = [part for part in env['PATH'].split(':') if '/neptune-agents-' not in part]
    env.update(HOME=str(home), EGUI_INSPECTION=endpoint, NEPTUNE_AGENT_TEST_LOG=str(output/'events.jsonl'),
               PATH=':'.join([str(fixtures), *path]))
    process = None
    log = (output/'app.log').open('w')
    def call(*args): return H['inspect'](CLIENT, endpoint, *map(str, args))
    def nodes(): return [node for _, node in call('tree')['Tree']['accesskit']['nodes']]
    def labels(): return [node['properties'].get('label', '') for node in nodes()]
    def rows(): return sorted(label for label in labels() if label.startswith('Agent '))
    def wait(predicate, what):
        end = time.monotonic() + 15
        seen = None
        while time.monotonic() < end:
            try:
                seen = rows()
                if predicate(seen): return seen
            except (OSError, ValueError, KeyError): pass
            time.sleep(.05)
        raise AssertionError(f'Timed out waiting for {what}; rows: {seen}')
    def shows(*expected):
        """The rows are exactly these `kind, state` prefixes, in any order."""
        wanted = sorted(expected)
        return wait(lambda seen: len(seen) == len(wanted) and all(row.startswith('Agent ' + prefix + ',') or row.startswith('Agent ' + prefix + ':') for row, prefix in zip(seen, wanted)), f'rows {wanted}')
    def text(value): call('text', value); call('key', 'Enter')
    def click(label):
        node = next(node for node in nodes() if node['properties'].get('label') == label)
        bounds = node['properties']['bounds']
        call('click', (bounds['x0']+bounds['x1'])/2, (bounds['y0']+bounds['y1'])/2)
    def fired(count):
        end = time.monotonic() + 15
        while time.monotonic() < end:
            if len(events()) >= count: return
            time.sleep(.05)
        raise AssertionError('The fixture did not start')
    def events(): return [json.loads(line) for line in (output/'events.jsonl').read_text().splitlines()] if (output/'events.jsonl').exists() else []
    checks = []
    try:
        process = subprocess.Popen([str(APP), '--data-root', str(data), '--size', '1100x700', '--cwd', str(data), '--no-restore'], env=env, stdout=log, stderr=log)
        H['wait_ready'](process, CLIENT, endpoint, 30)
        click('Toggle right panel')
        time.sleep(.4)  # The panel slides in.
        assert 'Files' in labels() and 'Agents' in labels(), 'the panel has both tabs'
        click('Agents')
        shows()
        call('screenshot', output/'empty.png')
        checks.append('the panel opens from the toolbar with Files and Agents tabs; no agent, no row')

        text('claude')
        fired(1)
        shows('Claude Code, Idle')
        text('fire UserPromptSubmit')
        shows('Claude Code, Working')
        text('fire PreToolUse Bash')
        text('fire PermissionRequest Bash')
        shows('Claude Code, Needs permission')
        # Another tool of the same batch finishing does not answer the request.
        text('fire PostToolUse Read')
        time.sleep(.6)
        shows('Claude Code, Needs permission')
        text('fire PostToolUse Bash size=6000000')
        shows('Claude Code, Working')
        text('fire PreToolUse AskUserQuestion permission_mode=bypassPermissions')
        shows('Claude Code, Asked a question')
        call('screenshot', output/'question.png')
        text('fire PostToolUse AskUserQuestion')
        shows('Claude Code, Working')
        text('fire PreToolUse ExitPlanMode')
        shows('Claude Code, Plan needs approval')
        text('fire PostToolUse ExitPlanMode')
        text('fire Stop')
        shows('Claude Code, Idle')
        checks.append('claude hooks: prompt, permission, question, plan, a 6 MB tool result and the end of the turn')

        # A request the agent decides itself is never one for a person.
        text('fire UserPromptSubmit')
        text('fire PermissionRequest Bash permission_mode=bypassPermissions')
        time.sleep(.8)
        shows('Claude Code, Working')
        # An interrupted turn fires no hook: the title alone says it rests.
        text('title ◐ Fix the build')
        wait(lambda seen: seen and seen[0].endswith(': Fix the build'), 'the conversation title')
        shows('Claude Code, Working')
        calm = time.monotonic()
        text('title ✳ Fix the build')
        time.sleep(1)
        held = rows()
        # The title must stay calm for two seconds before it is believed.
        if time.monotonic() - calm < 2:
            assert held and held[0].startswith('Agent Claude Code, Working,'), held
        shows('Claude Code, Idle')
        # Escape leaves a question without a hook, too.
        text('fire UserPromptSubmit')
        text('title ◐ Fix the build')
        text('fire PreToolUse AskUserQuestion')
        text('title ✳ Fix the build')
        shows('Claude Code, Asked a question')
        call('key', 'Escape')
        shows('Claude Code, Idle')
        checks.append('claude without a hook: an interrupted turn and a dismissed question come to rest by title and key')

        call('key', 'd', *(['--cmd'] if sys.platform == 'darwin' else ['--ctrl', '--shift']))
        time.sleep(.5)
        text('codex')
        fired(2)
        shows('Claude Code, Idle', 'Codex, Idle')
        text('fire UserPromptSubmit')
        shows('Claude Code, Idle', 'Codex, Working')
        text('fire PermissionRequest Bash')
        shows('Claude Code, Idle', 'Codex, Needs permission')
        text('fire Interrupt')
        shows('Claude Code, Idle', 'Codex, Idle')
        text('fire PreToolUse request_user_input')
        shows('Claude Code, Idle', 'Codex, Asked a question')
        call('screenshot', output/'two-agents.png')
        text('fire PostToolUse request_user_input')
        text('fire Stop')
        shows('Claude Code, Idle', 'Codex, Idle')
        # Codex says in its title when it is blocked, hooks or not.
        text('title [ ! ] Action Required')
        shows('Claude Code, Idle', 'Codex, Needs input')
        text('title ⠋ data')
        shows('Claude Code, Idle', 'Codex, Working')
        text('title data')
        shows('Claude Code, Idle', 'Codex, Idle')
        checks.append('codex hooks and titles: prompt, permission, interrupt, question, action required, spinner')

        # The toolbar marks a waiting agent while the list is out of view.
        text('fire PreToolUse request_user_input')
        shows('Claude Code, Idle', 'Codex, Asked a question')
        click('Files')
        time.sleep(.4)
        assert not rows(), 'the list leaves with its tab'
        call('screenshot', output/'files-with-waiting-agent.png')
        click('Agents')
        shows('Claude Code, Idle', 'Codex, Asked a question')
        # A row reveals its terminal.
        claude_row = next(label for label in rows() if label.startswith('Agent Claude'))
        click(claude_row)
        time.sleep(.3)
        call('resize', 640, 440)
        time.sleep(.5)
        call('screenshot', output/'narrow.png')
        call('resize', 1100, 700)
        time.sleep(.5)
        # The row clicked put the keyboard in Claude's terminal.
        text('exit')
        shows('Codex, Asked a question')
        checks.append('tabs switch, a row focuses its terminal, and an agent that exits leaves the list')

        for provider in events():
            wanted = {'claude': {'SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PermissionRequest', 'PostToolUse', 'PostToolUseFailure', 'Notification', 'Elicitation', 'ElicitationResult', 'Stop', 'StopFailure', 'PostCompact'},
                      'codex': {'SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PermissionRequest', 'PostToolUse', 'Stop', 'Interrupt'}}[provider['provider']]
            assert set(provider['hooks']) == wanted, provider
            assert provider['timeouts']['SessionStart'] is None, 'the session hook is unchanged'
            # Codex warns at every start about an interrupt hook asking for more than 3 s.
            assert all(limit == (3 if event == 'Interrupt' else 5) for event, limit in provider['timeouts'].items() if event != 'SessionStart'), provider
        saved = (data/'workspaces.json').read_text()
        for private in ('PRIVATE', 'Fix the build', 'needs_input'):
            assert private not in saved, f'{private} was saved'
        checks.append('injected hook sets are as documented; activity and hook input are not saved')
        click('Close window')
        process.wait(timeout=10)
        (output/'result.json').write_text(json.dumps({'status': 'passed', 'platform': sys.platform, 'features': ['inspection'], 'provider': 'deterministic fixtures', 'checks': checks, 'address': endpoint}, indent=2))
        print(output)
    except Exception:
        if process and process.poll() is None:
            call('screenshot', output/'failure.png')
        print(output)
        raise
    finally:
        if process: H['stop_owned'](process)
        log.close()

if __name__ == '__main__': main()
