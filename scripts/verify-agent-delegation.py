#!/usr/bin/env python3
"""Focused native proof that one CLI agent starts, hears, closes and reopens another.

Deterministic `claude` and `codex` fixtures run in an isolated instance. Each
starts Neptune's tool server the way its provider is told to and calls its
tools on request, fires the hooks injected for it, and answers every prompt it
is given, and asks a question first where its directory holds an `ask` file,
as a CLI does about a folder it has not seen. Tabs and the list of started
agents are read back through inspection. Real provider behaviour requires a separate installed-CLI check.
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
import json, os, subprocess, sys, time, tomllib, uuid
from pathlib import Path
args = sys.argv[1:]
provider = Path(sys.argv[0]).name
if '--help' in args:
    print('--no-daemon'); sys.exit(0)
LOG = os.environ['NEPTUNE_AGENT_TEST_LOG']
def note(**fields):
    with open(LOG, 'a') as log: log.write(json.dumps({'provider': provider, 'pid': os.getpid(), **fields}) + '\n')
valued = {'-c', '--settings', '--mcp-config', '--model', '-m', '--effort'}
if provider == 'claude':
    settings = json.loads(args[args.index('--settings')+1])
    hooks = {event: groups[0]['hooks'][0]['command'] for event, groups in settings['hooks'].items()}
    server = json.loads(args[args.index('--mcp-config')+1])['mcpServers']['neptune']
    allowed = settings['permissions']['allow']
    server_env = hook_env = dict(os.environ)
    effort = args[args.index('--effort')+1] if '--effort' in args else None
    ultracode = settings.get('ultracode', False)
    flag = '--resume'
else:
    hooks, server, policy, allowed, effort, ultracode = {}, {}, {}, [], None, False
    for index, arg in enumerate(args[:-1]):
        if arg != '-c': continue
        setting = args[index+1]
        if setting.startswith('model_reasoning_effort='):
            effort = tomllib.loads(setting)['model_reasoning_effort']
        if setting.startswith('hooks.'):
            for event, groups in tomllib.loads(setting)['hooks'].items():
                hooks[event] = groups[0]['hooks'][0]['command']
        for prefix, into in (('mcp_servers.neptune.', server), ('shell_environment_policy.set.', policy)):
            if setting.startswith(prefix):
                key, value = setting.removeprefix(prefix).split('=', 1)
                into[key] = tomllib.loads('value=' + value)['value']
    # Codex gives hooks and tool servers only the environment its configuration names.
    bare = {k: v for k, v in os.environ.items() if not k.startswith('NEPTUNE_AGENT_')}
    server_env = {**bare, **{k.removeprefix('env.'): v for k, v in server.items() if k.startswith('env.')}}
    hook_env = {**bare, **policy}
    flag = 'resume'
resumed = flag in args
session = args[args.index(flag)+1] if resumed else str(uuid.uuid4())
task = None
if args and not args[-1].startswith('-') and (len(args) < 2 or args[-2] not in valued | {flag}):
    task = args[-1]
choice = '--model' if provider == 'claude' else '-m'
model = args[args.index(choice)+1] if choice in args else None
answered = None
if os.path.exists('ask') and not resumed:
    # Asked before anything else runs, as a folder trust question is.
    print('Do you trust the files in this folder?\n  1. Yes, continue\n  2. No, quit', flush=True)
    answered = sys.stdin.readline().strip()
    if answered != '1': sys.exit(1)
def fire(event, tool=None, **fields):
    payload = {'session_id': session, 'cwd': os.getcwd(), 'hook_event_name': event,
               'transcript_path': '/nonexistent/transcript.jsonl', 'permission_mode': 'default'}
    if 'reply' in fields: payload['last_assistant_message'] = fields.pop('reply')
    payload.update(fields)
    if event == 'UserPromptSubmit': payload['prompt'] = 'PRIVATE PROMPT'
    if tool: payload.update(tool_name=tool, tool_input={'command': 'PRIVATE COMMAND'}, tool_use_id='tool-1')
    result = subprocess.run(hooks[event], shell=True, input=json.dumps(payload), text=True, capture_output=True, env=hook_env)
    print(f'FIRED {event} exit={result.returncode} out={len(result.stdout)}', flush=True)
tools = subprocess.Popen([server['command'], *server['args']], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=server_env)
count = 0
def rpc(method, params=None):
    global count
    count += 1
    tools.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': count, 'method': method, 'params': params or {}}) + '\n'); tools.stdin.flush()
    while True:
        answer = json.loads(tools.stdout.readline())
        if answer.get('id') == count: return answer
instructions = rpc('initialize', {'protocolVersion': '2025-06-18'})['result']['instructions']
tools.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n'); tools.stdin.flush()
offered = [tool['name'] for tool in rpc('tools/list')['result']['tools']]
fire('SessionStart', source='resume' if resumed else 'startup')
note(started=session, resumed=resumed, task=task, cwd=os.getcwd(), tools=offered, allowed=allowed, model=model, effort=effort, ultracode=ultracode, answered=answered,
     spawned='NEPTUNE_AGENT_SPAWNED' in hook_env and 'NEPTUNE_AGENT_SPAWNED' in server_env,
     told='reply_to_parent' in instructions, timeout=server.get('tool_timeout_sec'))
# Real agents take a paste whole; the markers show it arrived as one.
print('\033[?2004h' + provider.upper() + ' READY', flush=True)
slow = False
def turn(prompt):
    fire('UserPromptSubmit')
    if not slow:
        time.sleep(.2)
        fire('Stop', reply=f'{provider} did: {prompt.splitlines()[-1]}')
if task: turn(task)
while True:
    line = sys.stdin.readline()
    if not line: break
    pasted = '\x1b[200~' in line
    line = line.replace('\x1b[200~', '').replace('\x1b[201~', '').replace('\x1b', '').strip()
    words = line.split()
    if not words: continue
    if line == 'exit': break
    if line == 'slow': slow = True; print('SLOW', flush=True)
    elif words[0] == 'probe': note(probe=words[1])
    elif words[0] == 'fire':
        fields = dict(word.split('=', 1) for word in words[2:] if '=' in word)
        fire(words[1], next((word for word in words[2:] if '=' not in word), None), **fields)
    elif words[0] == 'call':
        arguments = json.loads(line.split(None, 2)[2])
        result = rpc('tools/call', {'name': words[1], 'arguments': arguments})
        said = result.get('result', {}).get('content', [{}])[0].get('text') or json.dumps(result.get('error'))
        note(call=words[1], text=said, failed=result.get('result', {}).get('isError', True))
        print('TOOL ' + said, flush=True)
    else:
        note(prompt=line, pasted=pasted)
        turn(line)
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts')
    options = parser.parse_args()
    options.output.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='agent-delegation-', dir=options.output)).resolve()
    data = output / 'data'; data.mkdir()
    home = output / 'home'; home.mkdir()
    tree = output / 'worktree'; tree.mkdir()
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
    def tabs(): return sorted(label for label in labels() if label.startswith('Terminal tab '))
    def wait(predicate, what):
        end = time.monotonic() + 20
        while time.monotonic() < end:
            try:
                value = predicate()
                if value: return value
            except (OSError, ValueError, KeyError, RuntimeError, StopIteration): pass
            time.sleep(.05)
        raise AssertionError(f'Timed out waiting for {what}')
    def events(): return [json.loads(line) for line in (output/'events.jsonl').read_text().splitlines()] if (output/'events.jsonl').exists() else []
    def started(): return [event for event in events() if 'started' in event]
    def text(value): call('text', value); call('key', 'Enter')
    def click(label, nth=0):
        found = [node for node in nodes() if node['properties'].get('label') == label]
        node = sorted(found, key=lambda node: (node['properties']['bounds']['x0'], node['properties']['bounds']['y0']))[nth]
        bounds = node['properties']['bounds']
        call('click', (bounds['x0']+bounds['x1'])/2, (bounds['y0']+bounds['y1'])/2)
    def focus(pane):
        click(f'Terminal tab {pane}')
        time.sleep(.3)
    def focused(pane):
        """The tab in view takes what is typed: its fixture notes the probe."""
        marker = str(time.monotonic_ns())
        text(f'probe {marker}')
        seen = wait(lambda: [event for event in events() if event.get('probe') == marker], 'the probe to be read')
        return seen[0]['pid'] == agents[pane]
    def tool(name, **arguments):
        before = len([event for event in events() if 'call' in event])
        text(f'call {name} {json.dumps(arguments)}')
        done = wait(lambda: [event for event in events() if 'call' in event][before:], f'{name} to answer')
        return done[0]
    def says(result, *parts):
        assert not result['failed'], result
        for part in parts: assert part in result['text'], result['text']
        return result['text']
    def refuses(result, part):
        assert result['failed'] and part in result['text'], result
    def saved_panes(): return [pane for workspace in json.loads((data/'workspaces.json').read_text())['workspaces'] for pane in workspace['panes']]
    def links(): return {pane['id']: pane.get('spawned_by') for pane in saved_panes() if pane.get('spawned_by')}
    def launch(restore, root=data):
        nonlocal process
        args = [str(APP), '--data-root', str(root), '--size', '1100x700']
        if not restore: args += ['--cwd', str(root), '--no-restore']
        process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
        H['wait_ready'](process, CLIENT, endpoint, 30)
    # The process of the fixture in each pane, as panes are numbered.
    agents = {}
    def opened(pane, count):
        wait(lambda: len(started()) >= count, f'agent {count} to start')
        agents[pane] = started()[count-1]['pid']
        return started()[count-1]
    checks = []
    try:
        launch(False)
        text('claude')
        parent = opened(1, 1)
        assert not parent['spawned'] and not parent['told'] and parent['task'] is None, parent
        assert parent['tools'] == ['link_pull_request', 'spawn_agent', 'wait_for_agent', 'send_agent_message', 'list_agents', 'close_agent', 'reopen_agent', 'press_agent_keys'], parent
        # Reading and linking are allowed outright; starting, messaging, closing and reopening an agent are the CLI's to decide.
        assert set(parent['allowed']) == {f'mcp__neptune__{name}' for name in ('link_pull_request', 'wait_for_agent', 'list_agents', 'reply_to_parent')}, parent
        says(tool('list_agents'), 'not started any agents')
        assert 'Started agents' not in labels()
        call('screenshot', output/'before.png')

        # Claude Code starts Codex with a task and a model, in a terminal out of view.
        refuses(tool('spawn_agent', agent='codex', prompt='x', model='--yolo'), 'model is not a model name')
        refuses(tool('spawn_agent', agent='claude', prompt='x', effort='ultra'), 'For ultracode, set ultra to true')
        refuses(tool('spawn_agent', agent='codex', prompt='x', effort='low', ultra=True), 'not both')
        says(tool('spawn_agent', agent='codex', prompt='Build the API', model='gpt-test', ultra=True),
             'Started Codex as agent 2 on gpt-test at ultra effort', str(data), 'It has taken its task', 'out of view')
        child = opened(2, 2)
        assert child['provider'] == 'codex' and child['spawned'] and child['told'] and child['model'] == 'gpt-test', child
        assert child['effort'] == 'ultra' and not child['ultracode'] and parent['effort'] is None and not parent['ultracode'], child
        assert child['task'].startswith('[Claude Code in another Neptune terminal started you'), child
        assert child['task'].endswith('\n\nBuild the API') and child['cwd'] == parent['cwd'], child
        assert child['tools'] == parent['tools'] + ['reply_to_parent'] and child['timeout'] == 330, child
        # No tab was added: the terminal that asked is still alone in view, and lists it in the toolbar.
        wait(lambda: 'Started agents' in labels(), 'the list of started agents')
        assert not tabs(), tabs()
        assert focused(1), 'the keyboard stays with the agent that started another'
        says(tool('wait_for_agent', agent_id=2), 'Agent 2 (Codex) finished its turn and replied:\n\ncodex did: Build the API')
        time.sleep(.4)
        call('screenshot', output/'started.png')
        checks.append('an agent starts the other provider with a task, a model and an effort in a terminal out of view and reads its reply')

        # The conversation goes on out of view: a message is typed for it as one paste.
        says(tool('send_agent_message', agent_id=2, message='Also add tests'), 'Sent to agent 2.')
        told = wait(lambda: [event for event in events() if event.get('prompt') == 'Also add tests'], 'the message to be typed')
        assert told[0]['pid'] == agents[2] and told[0]['pasted'], told
        says(tool('wait_for_agent', agent_id=2), 'codex did: Also add tests')
        says(tool('list_agents'), 'Agent 2 (Codex): idle')
        # An agent at rest may yet say more; nothing new is said to be so.
        says(tool('wait_for_agent', agent_id=2, timeout_seconds=1), 'Agent 2 (Codex) is idle and has said nothing new.', 'Nothing new after 1 seconds')
        assert not tabs(), tabs()
        checks.append('a follow-up message reaches the started agent as a paste and its answer returns')

        # Choosing it in the list gives it a tab and the keyboard.
        click('Started agents')
        wait(lambda: 'Codex' in labels(), 'the list to open')
        time.sleep(.5)  # The menu fades in.
        call('screenshot', output/'list.png')
        click('Codex')
        wait(lambda: tabs() == ['Terminal tab 1', 'Terminal tab 2'], 'the tab of the started agent')
        time.sleep(.3)
        assert focused(2), 'choosing a started agent opens its terminal'
        time.sleep(.3)
        call('screenshot', output/'opened.png')
        checks.append('the list of started agents opens the terminal of the one chosen, which has a tab from then on')

        # A turn that has not ended is waited for; a request to a person is news.
        text('slow')
        focus(1)
        says(tool('send_agent_message', agent_id=2, message='Refactor'), 'Sent to agent 2.')
        says(tool('wait_for_agent', agent_id=2, timeout_seconds=1), 'Agent 2 (Codex) is still working.', 'Call wait_for_agent again')
        focus(2)
        text('fire PermissionRequest Bash')
        focus(1)
        says(tool('wait_for_agent'), 'Agent 2 (Codex) is waiting for a person to answer a permission request in its terminal')
        refuses(tool('send_agent_message', agent_id=2, message='Hello?'), 'waiting for a person')
        refuses(tool('press_agent_keys', agent_id=2, keys=['enter']), 'Keys are pressed only for those')
        time.sleep(.4)
        call('screenshot', output/'waiting.png')
        focus(2)
        text('fire PostToolUse Bash')
        says(tool('reply_to_parent', message='Halfway there'), 'Delivered')
        checks.append('a started agent that needs a person is reported as such, is not typed over and has no keys pressed for it')

        # A started agent starts its own, also out of view; that one starts none.
        says(tool('spawn_agent', agent='claude', prompt='Write the docs'), 'Started Claude Code as agent 3')
        grandchild = opened(3, 3)
        assert grandchild['spawned'] and grandchild['model'] is None and grandchild['effort'] is None and not grandchild['ultracode'], grandchild
        assert grandchild['task'].startswith('[Codex in another Neptune terminal'), grandchild
        says(tool('wait_for_agent', agent_id=3), 'claude did: Write the docs')
        text('fire Stop reply=Refactored')
        assert len(tabs()) == 2, tabs()
        # The second tab lists it.
        click('Started agents', 1)
        wait(lambda: 'Claude Code' in labels(), 'the list on the second tab')
        time.sleep(.5)
        click('Claude Code')
        wait(lambda: len(tabs()) == 3, 'the third tab')
        time.sleep(.3)
        assert focused(3)
        refuses(tool('spawn_agent', agent='codex', prompt='More'), 'cannot start another')
        focus(1)
        says(tool('wait_for_agent', agent_id=2), 'Halfway there', 'Refactored')
        refuses(tool('wait_for_agent', agent_id=3), 'not an agent this one started')
        refuses(tool('send_agent_message', agent_id=3, message='Hi'), 'not an agent this one started')
        refuses(tool('reopen_agent', agent_id=3, message='Hi'), 'not an agent this one started')
        wait(lambda: links() == {2: 1, 3: 2}, 'the links to be saved')
        checks.append('started agents nest two deep, and an agent reaches only the agents it started')

        # A directory of its own, such as a worktree; one that is missing is refused.
        refuses(tool('spawn_agent', agent='claude', prompt='x', cwd=str(output/'missing')), 'cwd is not a directory')
        refuses(tool('spawn_agent', agent='gemini', prompt='x'), 'claude')
        says(tool('spawn_agent', agent='claude', prompt='Work apart', cwd=str(tree), model='opus', effort='xhigh', ultra=True),
             'agent 4 on opus at xhigh effort with ultracode on', str(tree))
        apart = opened(4, 4)
        assert apart['cwd'] == str(tree) and apart['model'] == 'opus' and apart['effort'] == 'xhigh' and apart['ultracode'] is True, apart
        says(tool('wait_for_agent', agent_id=4), 'claude did: Work apart')
        refuses(tool('reopen_agent', agent_id=4, message='x'), 'still open')
        says(tool('close_agent', agent_id=4), 'Closed agent 4 and its terminal.', 'reopen_agent')
        wait(lambda: 4 not in links() and not any(pane['id'] == 4 for pane in saved_panes()), 'the closed terminal to leave')
        refuses(tool('close_agent', agent_id=4), 'has already ended')
        assert len(tabs()) == 3, tabs()
        checks.append('a started agent works in the directory it is given and is closed on request')

        # Closed, it is opened again: the same conversation, directory and model, under a new number.
        says(tool('list_agents'), 'Agent 4 (Claude Code): ended')
        says(tool('reopen_agent', agent_id=4, message='Once more'), 'Reopened agent 4 with its conversation. It is agent 5 from now on', 'It has taken its task')
        again = opened(5, 5)
        assert again['resumed'] and again['started'] == apart['started'] and again['spawned'], again
        assert again['cwd'] == str(tree) and again['model'] == 'opus' and again['task'].endswith('\n\nOnce more'), again
        assert again['effort'] == 'xhigh' and again['ultracode'] is True, again
        says(tool('wait_for_agent', agent_id=5), 'claude did: Once more')
        refuses(tool('reopen_agent', agent_id=4, message='x'), 'not an agent this one started')
        wait(lambda: links() == {2: 1, 3: 2, 5: 1}, 'the reopened agent to be linked')
        assert len(tabs()) == 3, tabs()
        checks.append('an agent closed by the agent that started it is reopened with its conversation, directory, model, effort and ultracode')

        # Links return with their agents, in view or out of it; tasks and replies are not saved.
        saved = (data/'workspaces.json').read_text()
        for private in ('Build the API', 'Also add tests', 'did:', 'Halfway', 'PRIVATE', 'Once more'):
            assert private not in saved, f'{private} was saved'
        click('Close window')
        process.wait(timeout=10)
        launch(True)
        wait(lambda: len(started()) >= 9, 'four agents to resume')
        resumed = started()[5:]
        assert all(event['resumed'] and event['task'] is None for event in resumed), resumed
        by_session = {event['started']: event for event in resumed}
        assert not by_session[parent['started']]['spawned'], resumed
        assert all(by_session[event['started']]['spawned'] for event in (child, grandchild, apart)), resumed
        agents.update({pane: by_session[event['started']]['pid'] for pane, event in ((1, parent), (2, child), (3, grandchild), (5, apart))})
        wait(lambda: 'Started agents' in labels() and links() == {2: 1, 3: 2, 5: 1}, 'the list after reopening')
        assert len(tabs()) == 3, tabs()
        focus(1)
        says(tool('list_agents'), 'Agent 2 (Codex): idle', 'Agent 5 (Claude Code): idle')
        says(tool('send_agent_message', agent_id=5, message='After reopening'), 'Sent to agent 5.')
        says(tool('wait_for_agent', agent_id=5), 'claude did: After reopening')
        # One that exits without ever having had a tab takes its terminal with it.
        says(tool('send_agent_message', agent_id=5, message='exit'), 'Sent to agent 5.')
        says(tool('wait_for_agent', agent_id=5), 'Agent 5 (Claude Code) is no longer running: its agent exited', 'reopen_agent opens it again')
        wait(lambda: links() == {2: 1, 3: 2} and not any(pane['id'] == 5 for pane in saved_panes()), 'the unseen terminal to close')
        assert len(tabs()) == 3, tabs()
        call('screenshot', output/'restored.png')
        call('resize', 640, 440)
        time.sleep(.5)
        call('screenshot', output/'narrow.png')
        call('resize', 1100, 700)
        time.sleep(.5)
        checks.append('links and conversations between agents return after a close and reopen, out of view where they were; nothing they said is saved')

        # A person closes a started agent's terminal: the agent that started it hears, and opens it again.
        focus(3)
        call('key', 'w', *(['--cmd'] if sys.platform == 'darwin' else ['--ctrl', '--shift']))
        wait(lambda: len(tabs()) == 2, 'the closed tab to leave')
        focus(2)
        says(tool('wait_for_agent', agent_id=3), 'Agent 3 (Claude Code) is no longer running', 'reopen_agent opens it again')
        says(tool('reopen_agent', agent_id=3, message='Back to the docs'), 'Reopened agent 3', 'It is agent 6 from now on')
        back = opened(6, 10)
        assert back['resumed'] and back['started'] == grandchild['started'] and back['task'].endswith('\n\nBack to the docs'), back
        says(tool('wait_for_agent', agent_id=6), 'claude did: Back to the docs')
        assert len(tabs()) == 2, tabs()
        checks.append('a started agent whose terminal a person closed is reported and reopened with its conversation')

        # A started agent that exits is reported once and leaves the list; its shell stays,
        # and the agent it had started out of view gets a tab, since no agent answers for it now.
        text('exit')
        focus(1)
        says(tool('wait_for_agent', agent_id=2), 'Agent 2 (Codex) is no longer running: its agent exited', 'reopen_agent opens it again')
        wait(lambda: not links(), 'the links of the agent that left to end')
        wait(lambda: tabs() == ['Terminal tab 1', 'Terminal tab 2', 'Terminal tab 6'], 'the terminal left without a starter to get a tab')
        refuses(tool('send_agent_message', agent_id=2, message='Hi'), 'agent')
        call('screenshot', output/'exited.png')
        checks.append('a started agent that exits is reported and keeps its tab, and a terminal it left out of view gets one')
        click('Close window')
        process.wait(timeout=10)

        # A CLI that asks something before it takes its task: the agent that started it is told
        # what its terminal shows by the call that started it, and types the answer it is given.
        alone = output / 'alone'; alone.mkdir()
        guarded = output / 'guarded'; guarded.mkdir(); (guarded/'ask').write_text('')
        (alone/'config.toml').write_text((data/'config.toml').read_text())
        before = len(started())
        launch(False, alone)
        text('claude')
        opened(1, before + 1)
        says(tool('spawn_agent', agent='codex', prompt='Build the API'), 'Started Codex as agent 2')
        opened(2, before + 2)
        asked = says(tool('spawn_agent', agent='codex', prompt='Guarded work', cwd=str(guarded)),
                     'Started Codex as agent 3', 'is asking something first', 'Do you trust the files in this folder?', '1. Yes, continue', 'press_agent_keys')
        assert 'READY' not in asked and len(started()) == before + 2, asked
        says(tool('list_agents'), 'Agent 3 (Codex): asking something before it takes its task')
        refuses(tool('send_agent_message', agent_id=3, message='Hello?'), 'asking something before it takes its task')
        # Told once: a wait hears nothing new until it is answered.
        says(tool('wait_for_agent', agent_id=3, timeout_seconds=1), 'is asking something first', 'Nothing new after 1 seconds')
        click('Started agents')
        wait(lambda: 'Codex' in labels(), 'the list to open')
        time.sleep(.5)
        call('screenshot', output/'asking.png')
        call('key', 'Escape')
        time.sleep(.3)
        refuses(tool('press_agent_keys', agent_id=3, keys=['ctrl+c']), 'keys are up to')
        says(tool('press_agent_keys', agent_id=3, keys=['1', 'enter']), 'Pressed')
        guard = opened(3, before + 3)
        assert guard['answered'] == '1' and guard['cwd'] == str(guarded) and guard['task'].endswith('\n\nGuarded work'), guard
        says(tool('wait_for_agent', agent_id=3), 'codex did: Guarded work')
        refuses(tool('press_agent_keys', agent_id=3, keys=['enter']), 'Keys are pressed only for those')
        assert not tabs(), tabs()
        checks.append('a CLI that asks before taking its task is reported with what it shows by the call that started it, and is answered with the keys given')

        # A terminal alone in view has no tab: the toolbar carries the list,
        # and an agent moved to another workspace is opened there.
        click('Started agents')
        wait(lambda: 'Codex' in labels(), 'the list to open')
        time.sleep(.5)
        click('Codex')
        wait(lambda: len(tabs()) == 2, 'the tab of the started agent')
        click('New workspace')
        wait(lambda: 'home' in labels(), 'the second workspace')
        click('alone')
        wait(lambda: len(tabs()) == 2, 'the first workspace')
        focus(2)
        click('Command palette')
        time.sleep(.5)
        call('text', 'Move terminal to home')
        time.sleep(.5)
        call('key', 'Enter')
        # The view stays with the workspace the terminal left.
        wait(lambda: 'Started agents' in labels() and not tabs(), 'the list in the toolbar')
        time.sleep(.4)
        call('screenshot', output/'toolbar.png')
        click('Started agents')
        wait(lambda: 'Codex' in labels(), 'the list to open')
        time.sleep(.5)
        call('screenshot', output/'toolbar-list.png')
        # The one left in this workspace is listed before the one that moved.
        click('Codex', 1)
        time.sleep(.5)
        assert focused(2), 'an agent in another workspace is opened there'
        checks.append('a terminal alone in view lists its started agents in the toolbar, and one in another workspace is opened there')
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
