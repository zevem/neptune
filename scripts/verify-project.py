#!/usr/bin/env python3
"""Focused native proof that a project's lead is talked to, starts an agent and hears back.

A deterministic `claude` fixture runs in an isolated instance in both of its
parts. Started the way Neptune starts a lead it speaks the stream-json
handshake, starts the tool server its `--mcp-config` file names and calls
`spawn_agent` there when the person says so. Started in a terminal it is the
agent: it fires the hooks injected for it and answers its task, after a
question about its folder where that holds an `ask` file. The Project tab is
read back through inspection. The same run goes on to what a project keeps
(decisions, notes, instructions), its watches (a schedule, one the lead
proposes, a pull request read through a stand-in `gh`), a worktree for each
agent in a real git repository, and a restart in the middle of a turn. A second
data root shows the narrow-window sheet, a damaged and a later `project.json`,
and counts the frames of an idle project. Real provider behaviour requires a
separate installed-CLI check.
"""
import argparse
import json
import os
from pathlib import Path
import runpy
import shlex
import stat
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
H = runpy.run_path(str(ROOT / 'scripts/native-harness.py'))
APP = ROOT / 'target/debug/neptune'
CLIENT = ROOT / 'target/debug/neptune-inspect'
GH = runpy.run_path(str(ROOT / 'scripts/verify-pull-request-status.py'))['GH']
PULL = 'https://github.com/zevem/neptune/pull/'
FIXTURE = r'''#!/usr/bin/env python3
import json, os, queue, stat, subprocess, sys, threading, time, uuid
from pathlib import Path
args = sys.argv[1:]
if '--help' in args:
    print('--no-daemon'); sys.exit(0)
LOG = os.environ['PROJECT_TEST_LOG']
def note(**fields):
    with open(LOG, 'a') as log: log.write(json.dumps({'pid': os.getpid(), **fields}) + '\n')
class Tools:
    """Neptune's tool server, started as the CLI is told to start it."""
    def __init__(self, server, env):
        self.count = 0
        self.process = subprocess.Popen([server['command'], *server['args']], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
        self.instructions = self.rpc('initialize', {'protocolVersion': '2025-06-18'})['result']['instructions']
        self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n'); self.process.stdin.flush()
        self.offered = [tool['name'] for tool in self.rpc('tools/list')['result']['tools']]
    def rpc(self, method, params=None):
        self.count += 1
        self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': self.count, 'method': method, 'params': params or {}}) + '\n'); self.process.stdin.flush()
        while True:
            answer = json.loads(self.process.stdout.readline())
            if answer.get('id') == self.count: return answer
    def call(self, name, arguments):
        result = self.rpc('tools/call', {'name': name, 'arguments': arguments})
        said = result.get('result', {}).get('content', [{}])[0].get('text') or json.dumps(result.get('error'))
        return said, result.get('result', {}).get('isError', True)

def lead():
    """A project's lead: frames in, frames out, and tools called for real."""
    flag = next(arg for arg in args if arg.startswith(('--session-id=', '--resume=')))
    session = flag.split('=', 1)[1]
    config = Path(args[args.index('--mcp-config')+1])
    note(lead=session, resumed=flag.startswith('--resume='), argv=args, cwd=os.getcwd(),
         config_mode=stat.S_IMODE(config.stat().st_mode), folder_mode=stat.S_IMODE(config.parent.stat().st_mode),
         inherited=sorted(name for name in os.environ if name.startswith(('NEPTUNE_AGENT_', 'CLAUDE'))))
    lines = queue.Queue()
    def read():
        for line in sys.stdin: lines.put(line)
        lines.put(None)
    threading.Thread(target=read, daemon=True).start()
    def out(**frame): print(json.dumps(frame), flush=True)
    def answer(request, **body): out(type='control_response', response={'subtype': 'success', 'request_id': request, 'response': body})
    def event(**body): out(type='stream_event', event=body, session_id=session, parent_tool_use_id=None)
    def say(text, pause=0.0):
        event(type='content_block_start', index=0, content_block={'type': 'text', 'text': ''})
        for word in text.split(' '):
            event(type='content_block_delta', index=0, delta={'type': 'text_delta', 'text': word + ' '})
            if pause and interrupted(pause): return False
        out(type='assistant', message={'role': 'assistant', 'content': [{'type': 'text', 'text': text}]}, parent_tool_use_id=None, session_id=session)
        return True
    def interrupted(wait):
        """Takes an interrupt that arrives within `wait` seconds."""
        try: line = lines.get(timeout=wait)
        except queue.Empty: return False
        frame = json.loads(line) if line else {}
        if frame.get('request', {}).get('subtype') == 'interrupt':
            answer(frame['request_id'])
            return True
        return False
    def result(reason='completed', **more):
        out(type='result', subtype='success', is_error=False, terminal_reason=reason, api_error_status=None, total_cost_usd=0.0, session_id=session, **more)
    tools, turns, calls = None, 0, 0
    def use(name, arguments):
        nonlocal calls
        calls += 1
        call = f'toolu_{calls}'
        out(type='assistant', message={'role': 'assistant', 'content': [{'type': 'tool_use', 'id': call, 'name': f'mcp__neptune__{name}', 'input': arguments}]}, parent_tool_use_id=None, session_id=session)
        said, failed = tools.call(name, arguments)
        note(call=name, text=said, failed=failed)
        out(type='user', message={'role': 'user', 'content': [{'type': 'tool_result', 'tool_use_id': call, 'is_error': failed, 'content': [{'type': 'text', 'text': said}]}]}, parent_tool_use_id=None, session_id=session)
        return said
    def spawn(title, prompt, **where):
        return use('spawn_agent', {'agent': 'claude', 'title': title, 'prompt': prompt, **where})
    while True:
        line = lines.get()
        if line is None: break
        frame = json.loads(line)
        request = frame.get('request', {})
        if frame.get('type') == 'control_request' and request.get('subtype') == 'initialize':
            note(prompt=len(request.get('appendSystemPrompt', '')))
            # A hook's frame comes first, as it does from the CLI.
            out(type='system', subtype='hook_started', hook_name='SessionStart:startup', session_id=session)
            answer(frame['request_id'], models=[{'value': 'default', 'resolvedModel': 'fixture-model'}],
                   account={'subscriptionType': 'Fixture plan', 'apiProvider': 'firstParty'})
        elif frame.get('type') == 'control_request' and request.get('subtype') == 'mcp_status':
            server = json.loads(config.read_text())['mcpServers']['neptune']
            tools = Tools(server, {**os.environ, **server['env']})
            secrets = [server['env'][name] for name in ('NEPTUNE_AGENT_TOKEN', 'NEPTUNE_AGENT_RUN')]
            note(tools=tools.offered, server_env=sorted(server['env']), exposed=any(secret in arg for secret in secrets for arg in args))
            answer(frame['request_id'], mcpServers=[{'name': 'neptune', 'status': 'connected'}])
        elif frame.get('type') == 'control_request':
            answer(frame['request_id'])
        elif frame.get('type') == 'user':
            turns += 1
            text = frame['message']['content']
            words = text.rsplit('</neptune-events>', 1)[-1].rsplit('</project-state>', 1)[-1].strip()
            note(turn=turns, text=text, words=words)
            if words == 'go':
                said = spawn('auth', 'Refactor the PLUM module')
                say(f'LEAD-STARTED: {said}')
            elif words == 'guard':
                said = spawn('guarded', 'Check the QUINCE folder', cwd=os.path.join(os.environ['PROJECT_TEST_WORK'], 'guarded'))
                say(f'LEAD-GUARDED: {said}')
            elif words == 'close':
                say('LEAD-CLOSED: ' + use('close_agent', {'agent_id': 3}))
            elif words == 'decide':
                said = use('record_decision', {'decision': 'Store sessions in KIWI', 'why': 'It is deployed already'})
                say(f"LEAD-DECIDED: {said} {use('write_context', {'path': 'STATUS.md', 'content': 'LYCHEE migration: step 1 of 3'})}")
            elif words == 'note':
                say('LEAD-NOTED: ' + spawn('scribe', 'NOTE PAPAYA ripens in the cache layer'))
            elif words == 'tree':
                say('LEAD-TREE: ' + spawn('brancher', 'LINK ' + os.environ['PROJECT_TEST_PULL'], worktree='feature-a'))
            elif words == 'grove':
                say('LEAD-GROVE: ' + spawn('planter', 'Write the FIG file', worktree='feature-b'))
            elif words == 'nudge':
                say('LEAD-NUDGED: ' + use('send_agent_message', {'agent_id': 5, 'message': 'Say where the branch stands'}))
            elif words == 'propose':
                say('LEAD-PROPOSED: ' + use('add_subscription', {'title': 'Hourly sweep', 'trigger': {'schedule': {'every_minutes': 60}}, 'instruction': 'Sweep the ELDERBERRY queue'}))
            elif words == 'slow':
                if not say('LEAD-SLOW: ' + ' '.join(f'word{n}' for n in range(400)), 0.05):
                    result('aborted_streaming'); continue
            elif '<neptune-events>' in text:
                told = text.split('<neptune-events>', 1)[1].split('</neptune-events>', 1)[0]
                say('LEAD-HEARD: ' + ' '.join(line for line in told.splitlines() if line.startswith('[')))
            else:
                say('LEAD-PLAN: **one** agent would do `auth`. Say go when you want it started.')
            result()

def agent():
    """An agent in a terminal, as Neptune starts Claude Code for a task."""
    settings = json.loads(args[args.index('--settings')+1])
    hooks = {event: groups[0]['hooks'][0]['command'] for event, groups in settings['hooks'].items()}
    server = json.loads(args[args.index('--mcp-config')+1])['mcpServers']['neptune']
    resumed = '--resume' in args
    session = args[args.index('--resume')+1] if resumed else str(uuid.uuid4())
    valued = {'--settings', '--mcp-config', '--model', '--effort', '--resume'}
    task = args[-1] if args and not args[-1].startswith('-') and (len(args) < 2 or args[-2] not in valued) else None
    answered = None
    if os.path.exists('ask') and not resumed:
        # Asked before anything else runs, as a folder trust question is.
        print('Do you trust the files in this folder?\n  1. Yes, continue\n  2. No, quit', flush=True)
        answered = sys.stdin.readline().strip()[-1:]  # Keys pressed before the answer are not it.
        if answered != '1': sys.exit(1)
    def fire(event, **fields):
        payload = {'session_id': session, 'cwd': os.getcwd(), 'hook_event_name': event,
                   'transcript_path': '/nonexistent/transcript.jsonl', 'permission_mode': 'default'}
        if 'reply' in fields: payload['last_assistant_message'] = fields.pop('reply')
        payload.update(fields)
        if event == 'UserPromptSubmit': payload['prompt'] = 'PRIVATE PROMPT'
        done = subprocess.run(hooks[event], shell=True, input=json.dumps(payload), text=True, capture_output=True)
        print(f'FIRED {event} exit={done.returncode}', flush=True)
    tools = Tools(server, dict(os.environ))
    fire('SessionStart', source='resume' if resumed else 'startup')
    note(agent=session, task=task, cwd=os.getcwd(), tools=tools.offered, answered=answered,
         role=os.environ.get('NEPTUNE_AGENT_ROLE'), spawned='NEPTUNE_AGENT_SPAWNED' in os.environ)
    print('\033[?2004hCLAUDE READY', flush=True)
    def turn(prompt):
        fire('UserPromptSubmit')
        asked = prompt.splitlines()[-1]
        for word, tool, arguments in (('NOTE ', 'add_note', {'topic': 'Findings', 'content': asked[5:]}), ('LINK ', 'link_pull_request', {'url': asked[5:]})):
            if asked.startswith(word):
                said, failed = tools.call(tool, arguments)
                note(call=tool, text=said, failed=failed)
        time.sleep(.2)
        fire('Stop', reply=f'**AGENT-DID:** {asked}')
    if task: turn(task)
    while True:
        line = sys.stdin.readline()
        if not line: break
        line = line.replace('\x1b[200~', '').replace('\x1b[201~', '').replace('\x1b', '').strip()
        if line == 'exit': break
        if line: note(prompt=line); turn(line)

if '--input-format' in args and args[args.index('--input-format')+1] == 'stream-json': lead()
else: agent()
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts')
    options = parser.parse_args()
    options.output.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='project-', dir=options.output)).resolve()
    data = output / 'data'; data.mkdir()
    home = output / 'home'; home.mkdir()
    work = output / 'shop'; (work/'guarded').mkdir(parents=True); (work/'guarded'/'ask').write_text('')
    fixtures = output / 'bin'; fixtures.mkdir()
    scratch = output / 'tmp'; scratch.mkdir()
    for name, body in (('claude', FIXTURE), ('gh', GH)):
        path = fixtures/name; path.write_text(body); path.chmod(0o700)
    # A person's shell takes a moment to start where `slow` says so.
    (home/'.bashrc').write_text(f'export PATH={shlex.quote(str(fixtures))}:"$PATH"\nPS1="test $ "\n[ -e ~/slow ] && sleep 2\n')
    endpoint = H['free_endpoint']()
    # A run started from a Neptune terminal or a Claude Code session must reach neither.
    env = {key: value for key, value in os.environ.items() if not key.startswith(('NEPTUNE_', 'CLAUDE', 'GIT_'))}
    env.pop('WAYLAND_DISPLAY', None)
    parts = [part for part in env['PATH'].split(':') if '/neptune-agents-' not in part]
    env.update(HOME=str(home), EGUI_INSPECTION=endpoint, PROJECT_TEST_LOG=str(output/'events.jsonl'), PROJECT_TEST_PULL=PULL+'102',
               PROJECT_TEST_WORK=str(work), TMPDIR=str(scratch), PATH=':'.join([str(fixtures), *parts]))
    def git(*args, cwd=work):
        done = subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', *args], cwd=cwd, env=env, capture_output=True, text=True, timeout=20)
        assert done.returncode == 0, done.stderr
        return done.stdout
    # A real repository, so that an agent can be given a worktree of its own.
    git('init', '-q', '-b', 'main'); (work/'README.md').write_text('shop\n'); (work/'.gitignore').write_text('guarded/\n')
    git('add', '.'); git('commit', '-q', '-m', 'Start')
    def pulls(**states):
        # Written whole, so the stand-in never reads half a file.
        fresh = fixtures/'gh-states.json.new'
        fresh.write_text(json.dumps({number.lstrip('n'): pull for number, pull in states.items()}))
        fresh.replace(fixtures/'gh-states.json')
    def asked(number):
        log = fixtures/'gh.log'
        return [line for line in log.read_text().splitlines() if ['zevem', 'neptune', number] in json.loads(line)['asked']] if log.exists() else []
    process = None
    log = (output/'app.log').open('a')
    def call(*args): return H['inspect'](CLIENT, endpoint, *map(str, args))
    def nodes(): return [{**node['properties'], 'role': node.get('role'), 'flags': node.get('flags')} for _, node in call('tree')['Tree']['accesskit']['nodes']]
    def labels(): return [node.get('label', '') for node in nodes()]
    def shown():
        """Everything the window says, names and text alike."""
        return '\n'.join(str(node.get(key, '')) for node in nodes() for key in ('label', 'value'))
    def rows(part):
        """How many lines of the tab say `part`."""
        return len([node for node in nodes() if node['role'] == 'label' and part in str(node.get('value', ''))])
    def flags(label): return next(node for node in nodes() if node.get('label') == label)['flags']
    def tabs(): return sorted(label for label in labels() if label.startswith('Terminal tab '))
    def roster(agent):
        """The agent's pill in the strip over the chat, which is there without anything being opened."""
        return [label for label in labels() if label.startswith(f'Agent {agent}, Claude Code, ')]
    def listed(agent):
        """Its row in the list of all the agents, while that list is open over the chat."""
        return [label for label in labels() if label.startswith(f'Listed agent {agent}, Claude Code, ')]
    def agents_control(): return next(label for label in labels() if label.startswith('Agents, '))
    def press_agents_control():
        """The control at the strip's end opens the list and puts it away. Its name says how the agents stand and changes as they do, so it is found and pressed from one reading."""
        bounds = next(node for node in nodes() if node.get('label', '').startswith('Agents, '))['bounds']
        call('click', (bounds['x0']+bounds['x1'])/2, (bounds['y0']+bounds['y1'])/2)
    def wait(predicate, what, seconds=20):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            try:
                value = predicate()
                if value: return value
            except (OSError, ValueError, KeyError, RuntimeError, StopIteration, IndexError, subprocess.TimeoutExpired): pass
            time.sleep(.05)
        raise AssertionError(f'Timed out waiting for {what}')
    def says(*parts): return lambda: all(part in shown() for part in parts)
    def did(*parts):
        """What Neptune did for the lead is folded behind its turn's row once the turn has ended: the newest such row in view of the chat is pressed, then the tab is read again."""
        def read():
            if all(part in shown() for part in parts): return True
            found = nodes()
            # Only a row between what is pinned over the chat and the field: one scrolled out of view has something else where it would be pressed.
            # The strip of the agents is the last of what is pinned, and its control is on its one row.
            top = max(node['bounds']['y1'] for node in found if node.get('label') == 'Project details' or node.get('label', '').startswith('Agents, '))
            bottom = next(node for node in found if node.get('label') == 'Message the lead')['bounds']['y0']
            folded = [node['bounds'] for node in found if node.get('label', '').startswith('Show work, turn ') and top <= node['bounds']['y0'] and node['bounds']['y1'] <= bottom]
            if folded:
                bounds = max(folded, key=lambda bounds: bounds['y0'])
                call('click', (bounds['x0']+bounds['x1'])/2, (bounds['y0']+bounds['y1'])/2)
                time.sleep(.15)
            return False
        return read
    def events(): return [json.loads(line) for line in (output/'events.jsonl').read_text().splitlines()] if (output/'events.jsonl').exists() else []
    def noted(key, **same): return [event for event in events() if key in event and all(event.get(name) == value for name, value in same.items())]
    def click(label, nth=0):
        found = [node for node in nodes() if node.get('label') == label]
        bounds = sorted(found, key=lambda node: (node['bounds']['y0'], node['bounds']['x0']))[nth]['bounds']
        call('click', (bounds['x0']+bounds['x1'])/2, (bounds['y0']+bounds['y1'])/2)
    def write(label, words):
        click(label)
        time.sleep(.15)
        call('text', words)
    def send(words):
        write('Message the lead', words)
        call('key', 'Enter')
    def shot(name):
        time.sleep(.5)
        call('screenshot', output/f'{name}.png')
    def capture(name):
        """The same state at the default size and in a small window."""
        shot(name)
        call('resize', 640, 480)
        time.sleep(.6)
        call('screenshot', output/f'{name}-640x480.png')
        call('resize', 1180, 760)
        time.sleep(.6)
    def segment(name):
        """The chat, or one part of the project's details."""
        if name == 'Chat':
            click('Back to chat')
        else:
            if 'Back to chat' not in labels(): click('Project details')
            wait(lambda: name in labels(), 'the parts of the details')
            click(name)
        time.sleep(.3)
    def saved(): return (data/'workspaces.json').read_text()
    def configure(more=''):
        (data/'config.toml').write_text('shell = "/bin/bash"\nconfirm_close = false\nwarn_running_processes = false\n' + more)
    def launch(*args, size='1180x760'):
        nonlocal process
        process = subprocess.Popen([str(APP), '--data-root', str(data), '--size', size, *map(str, args)], env=env, stdout=log, stderr=log)
        H['wait_ready'](process, CLIENT, endpoint, 30)
    def fresh(): return ('--cwd', work, '--no-restore')
    def close():
        click('Close window')
        process.wait(timeout=10)
    def panel():
        """The Project tab in view: the panel is closed in a new window."""
        if 'Project' not in labels():
            click('Toggle right panel')
            wait(lambda: 'Project' in labels(), 'the Project tab')
            time.sleep(.4)  # The panel slides in.
        click('Project')
    def create(goal):
        wait(lambda: 'Project goal' in labels() and 'Start project' in labels(), 'the form')
        write('Project goal', goal)
        call('key', 'Enter')
        wait(says(goal, 'LEAD-PLAN'), 'the goal and the lead\'s reply')
    def frames():
        log.flush()
        counts = [json.loads(line)['count'] for line in (output/'app.log').read_text().splitlines() if line.startswith('{"') and '"operation":"frames"' in line]
        return counts[-1] if counts else 0
    def private(*words):
        """Nothing that was said is in the saved window or in the log."""
        text = saved()
        log.flush()
        logged = (output/'app.log').read_text()
        for word in words:
            assert word not in text, f'{word} was saved with the window'
            assert word not in logged, f'{word} was logged'
    checks = []
    try:
        configure()
        pulls(n101=dict(state='OPEN', checks='PENDING'), n102=dict(state='OPEN', checks='PENDING'))
        launch(*fresh())

        # The tab is there before any project is, and holds the form that makes one.
        panel()
        wait(lambda: 'Project goal' in labels() and 'Start project' in labels(), 'the form')
        capture('empty')
        assert not (data/'projects').exists() and not noted('lead'), 'nothing is made or started before the person asks'
        create('Tidy the MANGO checkout')
        folders = list((data/'projects').iterdir())
        assert len(folders) == 1 and stat.S_IMODE(folders[0].stat().st_mode) == 0o700, folders
        folder = folders[0]
        assert 'Project details' in labels() and 'Project menu' in labels() and 'Message the lead' in labels(), labels()
        checks.append('the Project tab offers its form, which makes a project and sends the goal as the first message')

        # The lead is Neptune's own process: no token on its command line, tools from a private file.
        started = noted('lead')[0]
        argv = started['argv']
        assert started['cwd'] == str(folder) and not started['resumed'], started
        config = argv[argv.index('--mcp-config')+1]
        assert Path(config).is_absolute() and Path(config).name == 'mcp.json' and not any('NEPTUNE_AGENT' in arg for arg in argv), argv
        # The file that named its credential was read as it started, and is gone.
        assert Path(config).parent.parent == scratch and not list(scratch.glob('neptune-lead-*')), config
        assert started['config_mode'] == 0o600 and started['folder_mode'] == 0o700, started
        assert '-p' not in argv and argv[argv.index('--tools')+1] == '' and argv[argv.index('--setting-sources')+1] == '', argv
        assert argv[argv.index('--permission-mode')+1] == 'dontAsk' and '--strict-mcp-config' in argv, argv
        assert started['inherited'] == ['CLAUDE_CODE_ENTRYPOINT', 'NEPTUNE_AGENT_ENDPOINT', 'NEPTUNE_AGENT_ROLE', 'NEPTUNE_AGENT_RUN', 'NEPTUNE_AGENT_TOKEN'], started
        offered = noted('tools')[0]
        assert not offered['exposed'] and offered['server_env'] == ['NEPTUNE_AGENT_ENDPOINT', 'NEPTUNE_AGENT_ROLE', 'NEPTUNE_AGENT_RUN', 'NEPTUNE_AGENT_TOKEN'], offered
        assert offered['tools'] == ['spawn_agent', 'send_agent_message', 'list_agents', 'agent_report', 'close_agent', 'reopen_agent', 'read_context', 'write_context', 'record_decision', 'add_subscription', 'list_subscriptions', 'remove_subscription', 'pull_request_status'], offered
        assert noted('prompt')[0]['prompt'] > 1000, 'the lead is told its role'
        first = noted('turn')[0]
        assert first['words'] == 'Tidy the MANGO checkout' and f'directory: {work}' in first['text'], first
        checks.append('the lead starts in its own folder with the tool server of a 0600 file that is removed once read, and its first turn carries the project\'s state')

        # The person says go: the lead calls the tool, Neptune writes what it did, the agent runs out of view.
        send('go')
        wait(did('Started agent 2 · Claude Code · auth', 'LEAD-STARTED'), 'the tool card and the lead\'s words')
        wait(lambda: any(label.endswith(': auth') for label in roster(2)), 'the agent in the strip')
        assert not tabs(), tabs()
        worker = wait(lambda: noted('agent'), 'the agent to start')[0]
        assert worker['cwd'] == str(work) and worker['role'] == 'member' and worker['spawned'], worker
        assert worker['task'].endswith('\n\nRefactor the PLUM module') and 'lead of a Neptune project' in worker['task'], worker
        assert worker['tools'][-2:] == ['read_context', 'add_note'], worker
        assert 'Agent 2 started' in noted('call')[0]['text'] and not noted('call')[0]['failed'], noted('call')
        checks.append('on the person\'s word the lead starts an agent through Neptune\'s tool; a card and a pill of the strip say so and no tab is added')

        # What the agent answered comes back by itself: a row of the chat, then a turn of the lead.
        wait(says('“auth” finished its turn', 'AGENT-DID: Refactor the PLUM module'), 'the agent\'s report')
        wait(says('LEAD-HEARD: [agent 2'), 'the lead\'s turn about it')
        # The card shows the report's words: its marks are styling, not text.
        assert '**AGENT-DID' in (folder/'chat.jsonl').read_text() and '**' not in shown(), 'the report keeps its marks'
        heard = [turn for turn in noted('turn') if '<neptune-events>' in turn['text']][0]
        assert '<agent-report agent="2" trust="agent">' in heard['text'] and heard['words'] == '', heard
        assert len(noted('lead')) == 1, 'one lead process serves every turn'
        # Its report is something to look at until its terminal is opened.
        wait(lambda: roster(2) == ['Agent 2, Claude Code, Ready for review: auth'], 'the agent at rest')
        assert any(label.startswith('Show changes of agent 2') for label in labels()), 'the result row offers its actions'
        capture('chat-roster')
        checks.append('the agent\'s reply returns as an event row with Open terminal and Changes, and the lead takes a turn about it without being asked')

        # An agent that asks about its folder waits for the person, and only the person.
        send('guard')
        wait(did('Started agent 3 · Claude Code · guarded'), 'the second card')
        wait(lambda: 'Open terminal of agent 3' in labels() and 'Needs you' in shown(), 'the row that needs the person', 30)
        wait(says('Needs your answer'), 'what it waits for')
        assert len(noted('agent')) == 1, 'the agent has not taken its task'
        wait(says('LEAD-HEARD: [agent 3'), 'the lead to be told', 30)
        capture('needs-you')
        began = time.monotonic()
        click('Open terminal of agent 3')
        wait(lambda: 'Terminal tab 3' in tabs(), 'the terminal of the agent that asks')
        time.sleep(.4)
        call('screenshot', output/'needs-you-opened.png')
        # The row that sent the person there stays while its terminal redraws what it asks.
        assert 'Open terminal of agent 3' in labels(), 'the row left as its terminal was opened'
        assert roster(3) == ['Agent 3, Claude Code, Needs you: guarded'], labels()
        # Its terminal in view shows the question anew, long after it was started. It is the same wait:
        # the row returns, and the lead is not told again.
        call('key', 'ArrowDown')
        wait(lambda: 'Open terminal of agent 3' in labels(), 'the row to return once the terminal is still')
        time.sleep(max(0, 24 - (time.monotonic() - began)))
        assert 'Open terminal of agent 3' in labels() and len(noted('agent')) == 1
        assert rows('“guarded” asks something before starting') == 1 and not rows('has not started yet'), shown()
        assert len([turn for turn in noted('turn') if '[agent 3' in turn['text']]) == 1, 'the lead heard of the question once'
        call('text', '1')
        call('key', 'Enter')
        guarded = wait(lambda: noted('agent')[1:], 'the answered agent to start')[0]
        assert guarded['answered'] == '1' and guarded['cwd'] == str(work/'guarded') and guarded['task'].endswith('Check the QUINCE folder'), guarded
        wait(lambda: 'Open terminal of agent 3' not in labels(), 'the row to leave')
        wait(says('AGENT-DID: Check the QUINCE folder'), 'its report')
        wait(says('LEAD-HEARD: [agent 3 "guarded" finished'), 'the lead to hear of it')
        checks.append('an agent that asks before its task is pinned over the chat; Open shows its terminal, where the answer is typed')

        # The lead closes an agent whose terminal the person opened: the terminal stays, as a tab of its own.
        send('close')
        wait(did('Agent 3 left the project. Its terminal stays open', 'LEAD-CLOSED'), 'the card of the agent that left')
        wait(lambda: not roster(3), 'the agent to leave the strip')
        assert tabs() == ['Terminal tab 1', 'Terminal tab 3'] and not noted('call', call='close_agent')[0]['failed'], tabs()
        assert Path(f"/proc/{guarded['pid']}").exists(), 'the terminal the person opened was closed with its agent'
        checks.append('close_agent on an agent whose terminal the person opened releases it: the tab and its CLI stay')

        # Pause holds what the lead would do by itself; the person stops a turn.
        click('Project menu')
        wait(lambda: 'Pause project' in labels(), 'the menu')
        click('Pause project')
        wait(lambda: 'Resume project' in labels() and 'Pause project' not in labels(), 'the project to pause')
        wait(lambda: '"paused": true' in saved() or '"paused":true' in saved(), 'the pause to be saved')
        send('go')
        wait(says('The lead\'s spawn_agent request was refused: the project is paused', 'This project is paused'), 'the refusal while paused')
        assert noted('call')[-1]['failed'] and len(noted('agent')) == 2, noted('call')[-1]
        click('Resume project')
        wait(lambda: 'Resume project' not in labels(), 'the project to resume')
        send('slow')
        wait(lambda: 'Stop the lead' in labels() and 'LEAD-SLOW' in shown(), 'a turn that streams')
        time.sleep(.3)
        call('screenshot', output/'streaming.png')
        click('Stop the lead')
        wait(says('You stopped the lead.'), 'the turn to stop')
        assert len(noted('lead')) == 1, 'the lead is kept after an interrupt'
        checks.append('Pause and Resume toggle and are saved, a paused project starts nothing and says why, and Stop ends a streaming turn without ending the lead')

        # What the project keeps: the lead records, the person instructs, and the next agent is handed both.
        send('decide')
        wait(did('Recorded a decision: Store sessions in KIWI', 'Updated STATUS.md', 'LEAD-DECIDED'), 'the cards of what the lead wrote')
        context = folder/'context'
        decisions = (context/'DECISIONS.md').read_text()
        assert 'Store sessions in KIWI' in decisions and 'Why: It is deployed already' in decisions and ' UTC · lead' in decisions, decisions
        assert (context/'STATUS.md').read_text().strip() == 'LYCHEE migration: step 1 of 3'
        segment('Context')
        wait(lambda: {'Open DECISIONS.md', 'Open STATUS.md', 'Open INDEX.md', 'Project instructions'} <= set(labels()), 'the files in the Context segment')
        write('Project instructions', 'Mention GUAVA in every report.')
        time.sleep(.3)
        call('screenshot', output/'context-editing.png')
        click('Save instructions')
        wait(lambda: (context/'INSTRUCTIONS.md').read_text().strip() == 'Mention GUAVA in every report.', 'the instructions to be saved')
        wait(lambda: 'Open INSTRUCTIONS.md' in labels(), 'the instructions among the files')
        assert all(stat.S_IMODE(path.stat().st_mode) == 0o600 for path in context.iterdir() if path.is_file()), list(context.iterdir())
        segment('Chat')
        send('note')
        wait(did('Started agent 4 · Claude Code · scribe', 'LEAD-NOTED'), 'the agent that takes a note')
        scribe = wait(lambda: noted('agent')[2:], 'the note taker to start')[0]
        brief = scribe['task']
        assert 'Mention GUAVA in every report.' in brief and 'Store sessions in KIWI' in brief and 'LYCHEE migration' in brief, brief
        assert brief.index('GUAVA') < brief.index('NOTE PAPAYA') and str(context) in brief, brief
        wait(lambda: noted('call', call='add_note'), 'the agent\'s note')
        assert not noted('call', call='add_note')[0]['failed'], noted('call', call='add_note')
        written = (context/'notes'/'findings.md').read_text()
        assert 'PAPAYA ripens in the cache layer' in written and '— agent 4 "scribe"' in written, written
        # The lead is told what the person wrote since its last turn.
        assert 'Mention GUAVA in every report.' in noted('turn', words='note')[0]['text'], noted('turn', words='note')[0]
        wait(says('AGENT-DID: NOTE PAPAYA'), 'the note taker\'s report')
        segment('Context')
        wait(lambda: 'Open notes/findings.md' in labels(), 'the note among the files')
        capture('context')
        click('Actions for notes/findings.md')
        wait(lambda: 'Reveal in file manager' in labels() and 'Delete…' in labels(), 'the menu of a file')
        shot('context-file-menu')
        call('key', 'Escape')
        segment('Chat')
        checks.append('the lead\'s decision and status, the person\'s instructions and an agent\'s note are files of the project, listed under Context, and the next agent\'s brief holds them')

        # An agent that edits gets a worktree of its own, on a branch of its own.
        send('tree')
        wait(did('Started agent 5 · Claude Code · brancher · in its own worktree, on feature-a', 'LEAD-TREE'), 'the agent in a worktree', 40)
        send('grove')
        wait(did('Started agent 6 · Claude Code · planter · in its own worktree, on feature-b', 'LEAD-GROVE'), 'the second agent in a worktree', 40)
        brancher, planter = wait(lambda: len(noted('agent')) >= 5 and noted('agent')[3:5], 'both to start')
        trees = work.with_name('shop.worktrees')
        assert {brancher['cwd'], planter['cwd']} == {str(trees/'feature-a'), str(trees/'feature-b')}, (brancher['cwd'], planter['cwd'])
        worktrees = git('worktree', 'list', '--porcelain')
        assert all(f'worktree {trees/name}\n' in worktrees and f'branch refs/heads/{name}\n' in worktrees for name in ('feature-a', 'feature-b')), worktrees
        assert git('rev-parse', '--abbrev-ref', 'HEAD', cwd=trees/'feature-a').strip() == 'feature-a'
        assert 'You work in a git worktree of your own: branch feature-a' in next(agent for agent in (brancher, planter) if agent['cwd'].endswith('feature-a'))['task']
        assert git('status', '--porcelain') == '', 'the worktrees are beside the repository, not in it'
        checks.append('two agents asked for with worktrees start on branches of their own in checkouts of their own, beside a real git repository')

        # A pull request an agent links is followed without being asked; the person adds one, and a schedule.
        wait(lambda: noted('call', call='link_pull_request') and asked(102), 'the linked pull request to be read')
        segment('Watches')
        wait(lambda: 'Add watch' in labels(), 'the Watches segment')
        click('Add watch')
        wait(lambda: 'Watch title' in labels() and 'How often the watch runs' in labels(), 'the form of a watch')
        call('text', 'Nightly')
        # A number of minutes is typed once it is asked for.
        click('How often the watch runs')
        wait(lambda: 'Every day' in labels() and 'Custom…' in labels(), 'how often a watch may run')
        click('Custom…')
        wait(lambda: 'Minutes between runs' in labels(), 'the field of minutes')
        write('Minutes between runs', '5')
        write('What the watch does', 'Check the DURIAN build')
        time.sleep(.3)
        call('screenshot', output/'watch-form-too-often.png')
        refused = flags('Add the watch')
        click('Minutes between runs')
        call('key', 'a', '--ctrl'); call('text', '15')
        time.sleep(.3)
        shot('watch-form')
        assert flags('Add the watch') != refused, 'a schedule more often than every 15 minutes can be added'
        click('Add the watch')
        wait(lambda: 'Actions for watch Nightly' in labels(), 'the schedule in the list')
        click('Add watch')
        wait(lambda: 'Watch title' in labels(), 'the form again')
        call('text', 'Release')
        click('Pull request')
        wait(lambda: 'Pull request address' in labels(), 'the address field')
        write('Pull request address', PULL+'101')
        click('Add the watch')
        wait(lambda: 'Actions for watch Release' in labels(), 'the pull request in the list')
        wait(lambda: asked(101), 'the watched pull request to be read')
        def kept(): return json.loads((folder/'project.json').read_text())
        wait(lambda: [watch['title'] for watch in kept()['subscriptions']] == ['PR #102', 'Nightly', 'Release'], 'the watches to be saved')
        assert kept()['version'] == 1 and stat.S_IMODE((folder/'project.json').stat().st_mode) == 0o600, kept()
        click('Actions for watch Nightly')
        wait(lambda: 'Run now' in labels() and 'Pause' in labels() and 'Delete…' in labels(), 'the menu of a watch')
        click('Run now')
        segment('Chat')
        wait(says('Watch “Nightly” fired · run by you', 'LEAD-HEARD: [watch "Nightly" fired'), 'the run the person asked for')
        fire = [turn for turn in noted('turn') if '[watch "Nightly" fired' in turn['text']][0]
        assert 'Check the DURIAN build' in fire['text'] and '<instruction saved=' in fire['text'] and fire['words'] == '', fire
        # What a pull request does wakes the lead: first sight says nothing, failing checks do.
        time.sleep(1)
        assert not rows('Pull request zevem/neptune#'), 'a pull request seen for the first time is not news'
        pulls(n101=dict(state='OPEN', checks='FAILURE'), n102=dict(state='OPEN', checks='PENDING'))
        wait(says('LEAD-HEARD: [pull request ' + PULL + '101'), 'the lead to be woken by failing checks', 45)
        woke = [turn for turn in noted('turn') if '[pull request ' + PULL + '101' in turn['text']][0]
        assert 'its checks are failing' in woke['text'] and woke['words'] == '' and rows('Pull request zevem/neptune#101') == 1, woke
        # The one an agent linked says whose it is.
        pulls(n101=dict(state='OPEN', checks='FAILURE'), n102=dict(state='OPEN', checks='FAILURE', unresolved=1))
        wait(says('LEAD-HEARD: [pull request ' + PULL + '102'), 'the lead to hear of the agent\'s pull request', 45)
        told = [turn for turn in noted('turn') if '[pull request ' + PULL + '102' in turn['text']][0]
        assert 'of agent 5 "brancher"' in told['text'] and rows('Pull request zevem/neptune#102') == 1, told
        # Its own row offers what the agent's row would, which is long out of view.
        def offered(ask): return [label for label in labels() if label.startswith(f'Ask agent 5 to {ask}, watch ')]
        wait(lambda: offered('fix its failing checks') and offered('address its review comments'), 'the follow-ups on the pull request\'s row')
        assert not [label for label in labels() if label.startswith('Ask agent ') and ', watch ' in label and not label.startswith('Ask agent 5 ')], labels()
        capture('pull-request-row-follow-up')
        click(offered('fix its failing checks')[0])
        wait(says('You asked agent 5 to fix the failing checks of zevem/neptune#102'), 'the card of the follow-up')
        wait(lambda: any('are failing' in event['prompt'] for event in noted('prompt') if isinstance(event['prompt'], str)), 'the agent to take the follow-up')
        wait(lambda: rows('AGENT-DID: The checks of your pull request') == 1 and [turn for turn in noted('turn') if '[agent 5 "brancher" finished' in turn['text']], 'its report of the fix to reach the lead')
        wait(says('LEAD-HEARD: [agent 5 "brancher" finished'), 'the lead\'s words about the fix')
        assert len(noted('lead')) == 1 and 'gh.log' not in saved()
        # Read from the saved chat: a row of the lead's would be folded with its turn's work.
        assert 'Took a message for agent 5' not in (folder/'chat.jsonl').read_text(), 'the person\'s follow-up was written as the lead\'s message'
        # The newest row about the agent offers the same, once it rests again.
        send('nudge')
        wait(did('Took a message for agent 5', 'AGENT-DID: Say where the branch stands'), 'the agent\'s answer to its lead\'s message')
        wait(lambda: len([turn for turn in noted('turn') if '[agent 5 "brancher" finished' in turn['text']]) == 2, 'the lead to hear of it')
        wait(lambda: 'Ask agent 5 to fix its failing checks' in labels() and 'Ask agent 5 to address its review comments' in labels(), 'the follow-ups on the agent\'s row')
        capture('result-row-follow-up')
        wait(did('Took a message for agent 5'), 'the card of the lead\'s message, unfolded')
        assert rows('Took a message for agent 5') == 1
        time.sleep(1)
        # A schedule the lead proposes runs only once the person allows it.
        send('propose')
        wait(says('The lead proposes a watch', '“Hourly sweep”', 'Sweep the ELDERBERRY queue', 'LEAD-PROPOSED'), 'the proposal')
        allow = wait(lambda: [label for label in labels() if label.startswith('Allow watch ')], 'the Allow of the card')[0]
        assert 'proposed' in noted('call', call='add_subscription')[0]['text'], noted('call', call='add_subscription')
        proposed = wait(lambda: next(watch for watch in kept()['subscriptions'] if watch['title'] == 'Hourly sweep'), 'the proposal to be saved')
        assert not proposed['allowed'] and not proposed.get('next'), proposed
        capture('allow-card')
        segment('Watches')
        wait(lambda: 'Allow watch Hourly sweep' in labels() and 'Decline watch Hourly sweep' in labels(), 'the proposal in the list')
        # A row says how soon and how long ago, not a time of day.
        wait(says('Every 15 min · Next in 1', 'Last ran '), 'the schedule\'s times')
        assert ' UTC' not in shown(), shown()
        capture('watches')
        segment('Chat')
        time.sleep(.3)
        click(allow)
        wait(says('You allowed the watch “Hourly sweep”: it runs every 1 h.'), 'the notice of the leave given')
        wait(lambda: next(watch for watch in kept()['subscriptions'] if watch['title'] == 'Hourly sweep')['allowed'], 'the leave to be saved')
        checks.append('a schedule the person adds runs when asked, a pull request read through gh wakes the lead when its checks fail and offers Fix CI on its own row and on its agent\'s, and a schedule the lead proposes waits for Allow')

        # Nothing that was said is in the saved window.
        words = ('MANGO', 'PLUM', 'QUINCE', 'LEAD-', 'AGENT-DID', 'KIWI', 'LYCHEE', 'GUAVA', 'PAPAYA', 'DURIAN', 'ELDERBERRY', 'Nightly', 'Hourly', 'scribe', 'brancher')
        private(*words, 'auth', 'go"', 'lead-')
        state = json.loads(saved())
        assert [project['name'] for project in state['projects']] == ['shop'], state.get('projects')
        assert sorted(pane.get('project') for workspace in state['workspaces'] for pane in workspace['panes'] if pane.get('project')) == [1, 1, 1, 1], saved()
        checks.append('the saved window holds the project and its members and none of the words; the log holds none either')

        # Neptune closes in the middle of a turn and opens again on the same data.
        session = noted('lead')[0]['lead']
        send('slow')
        wait(lambda: 'Stop the lead' in labels() and 'LEAD-SLOW' in shown(), 'a second turn that streams')
        close()
        wait(lambda: not Path(f"/proc/{noted('lead')[0]['pid']}").exists(), 'the lead to end with the window')
        chat = (folder/'chat.jsonl').read_text()
        assert chat.startswith('{"neptune_chat":1}\n') and stat.S_IMODE((folder/'chat.jsonl').stat().st_mode) == 0o600, chat[:80]
        assert 'Tidy the MANGO checkout' in chat and 'LEAD-PLAN' in chat and 'Store sessions in KIWI' in chat
        state = json.loads((folder/'project.json').read_text())
        assert session in json.dumps(state), 'the lead\'s conversation is kept with the project'
        # Two runs of the schedule came due while Neptune was closed.
        for watch in state['subscriptions']:
            if watch['title'] == 'Nightly': watch['next'] = int(time.time()) - 1000
        (folder/'project.json').write_text(json.dumps(state))
        before = len(noted('agent'))
        def reports(): return (folder/'chat.jsonl').read_text().count('finished its turn')
        finished = reports()
        # The shells that open the agents' conversations again are not there at once.
        (home/'slow').write_text('')
        opened = time.monotonic()
        launch()
        panel()
        wait(says('Tidy the MANGO checkout', 'LEAD-DECIDED', 'Neptune closed while the lead was replying.'), 'the chat and the notice of the turn that was cut')
        assert rows('Neptune closed while the lead was replying.') == 1 and 'Stop the lead' not in labels()
        wait(lambda: all(roster(agent) for agent in (2, 4, 5, 6)), 'the project\'s agents in the strip again')
        assert any(label.endswith(': auth') for label in roster(2)) and not roster(3), labels()
        assert tabs() == ['Terminal tab 1', 'Terminal tab 3'], tabs()
        again = wait(lambda: len(noted('agent')) >= before + 5 and noted('agent')[before:], 'the agents\' CLIs to be resumed')
        assert {agent['agent'] for agent in again} == {agent['agent'] for agent in noted('agent')[:before]}, again
        assert all(agent['task'] is None for agent in again), again
        # An agent that rests again as it was left is not news: the lead is not started for it.
        wait(lambda: all(roster(agent)[0].split(', ')[2].startswith('Idle') for agent in (2, 4, 5, 6)), 'the resumed agents at rest')
        time.sleep(4)
        assert len(noted('lead')) == 1 and reports() == finished, 'the lead was woken by agents that only opened again'
        (home/'slow').unlink()
        capture('restored')
        send('What is left?')
        wait(lambda: len(noted('lead')) == 2, 'the lead to start for the next turn')
        resumed = noted('lead')[1]
        assert resumed['resumed'] and resumed['lead'] == session and resumed['cwd'] == str(folder), resumed
        wait(lambda: noted('turn', words='What is left?'), 'the turn after the restart')
        # One late run comes half a minute after opening, and says what was missed.
        wait(lambda: rows('Watch “Nightly” fired · schedule · due ') == 1 and 'missed 2 runs while Neptune was closed' in shown(), 'the run that was missed', max(5, 50 - (time.monotonic() - opened)))
        wait(says('LEAD-HEARD: [watch "Nightly" fired · schedule · due '), 'the lead\'s turn about it')
        assert rows('Watch “Nightly” fired · schedule') == 1 and len(noted('lead')) == 2
        later = next(watch for watch in json.loads((folder/'project.json').read_text())['subscriptions'] if watch['title'] == 'Nightly')
        assert later['next'] > time.time(), later
        checks.append('closed in the middle of a turn and opened again: the chat is shown with a notice of the cut turn, the agents are listed and their CLIs resumed, the lead is resumed with its conversation on the next turn, and a schedule that came due runs once, late')

        # A new chat sets the old one aside and starts the lead on a conversation of its own.
        click('Project menu')
        wait(lambda: 'New chat' in labels() and 'Reveal saved files' in labels() and 'Remove project…' in labels(), 'the menu')
        time.sleep(.4)
        call('screenshot', output/'menu.png')
        click('New chat')
        wait(says("New chat. The earlier one is kept with the project's saved files"), 'the notice of the new chat')
        wait(lambda: 'MANGO' not in shown() and 'LEAD-DECIDED' not in shown(), 'the earlier chat to leave the tab')
        wait(lambda: 'Tidy the MANGO checkout' in (folder/'chat.1.jsonl').read_text(), 'the earlier chat in the folder')
        send('Begin again')
        wait(lambda: len(noted('lead')) == 3, 'a lead for the new chat')
        anew = noted('lead')[2]
        assert not anew['resumed'] and anew['lead'] != session, anew
        opening = wait(lambda: noted('turn', words='Begin again'), 'its first turn')[0]
        assert 'Store sessions in KIWI' in opening['text'] and 'Mention GUAVA in every report.' in opening['text'], opening
        assert all(roster(agent) for agent in (2, 4, 5, 6)), 'a new chat keeps the project\'s agents'
        checks.append('New chat keeps the earlier chat in the folder and starts a lead that is handed what the project keeps')

        # Removed, the project lets its agents go on as tabs, its lead ends and its folder is deleted.
        private(*words)
        click('Project menu')
        wait(lambda: 'Remove project…' in labels(), 'the menu')
        click('Remove project…')
        wait(lambda: 'Remove' in labels() and 'Cancel' in labels(), 'the confirmation')
        time.sleep(.4)
        call('screenshot', output/'remove.png')
        click('Remove')
        wait(lambda: tabs() == [f'Terminal tab {tab}' for tab in (1, 2, 3, 4, 5, 6)], 'the agents to get tabs')
        wait(lambda: 'Project goal' in labels(), 'the form again')
        wait(lambda: 'projects' not in json.loads(saved()), 'the project to leave the saved window')
        wait(lambda: not folder.exists(), 'the project\'s folder to be deleted')
        assert (trees/'feature-a').is_dir(), 'a removed project keeps its agents\' worktrees'
        lead = noted('lead')[2]['pid']
        wait(lambda: not Path(f'/proc/{lead}').exists(), 'the lead\'s process to end')
        time.sleep(.4)
        call('screenshot', output/'removed.png')
        checks.append('a removed project gives its agents tabs, ends its lead, deletes its folder and leaves the saved window')
        close()

        # A second window's worth of data: what is idle stays idle, and what is damaged is kept.
        data = output / 'data-2'; data.mkdir()
        configure()
        launch(*fresh(), '--diagnostics')
        panel()
        create('Sort the NECTARINE crates')
        folder = next((data/'projects').iterdir())
        segment('Watches')
        click('Add watch')
        wait(lambda: 'Watch title' in labels(), 'the form of a watch')
        call('text', 'Weekly')
        click('How often the watch runs')
        wait(lambda: 'Every 15 minutes' in labels(), 'how often a watch may run')
        click('Every 15 minutes')
        write('What the watch does', 'Count the OLIVE jars')
        click('Add the watch')
        wait(lambda: 'Actions for watch Weekly' in labels(), 'the schedule in the list')
        wait(lambda: any(watch.get('next') for watch in json.loads((folder/'project.json').read_text())['subscriptions']), 'its next run to be saved')
        # An idle project with a schedule ahead draws nothing by itself: nothing is asked of the window meanwhile.
        idle = {}
        for name in ('Watches', 'Chat'):
            segment(name)
            time.sleep(3)
            began, at = frames(), time.monotonic()
            time.sleep(12)
            idle[name] = round((frames() - began) / (time.monotonic() - at), 1)
        assert max(idle.values()) < 6, f'an idle project keeps the window drawing: {idle} frames a second'
        checks.append(f'an idle project with a schedule ahead drew {idle["Chat"]} frames a second with its chat in view and {idle["Watches"]} with its watches')
        # An agent, so that the sheet has a strip and a list to open over its chat.
        send('go')
        wait(did('Started agent 2 · Claude Code · auth', 'LEAD-HEARD: [agent 2'), 'an agent of the second project and its report')
        wait(lambda: roster(2) == ['Agent 2, Claude Code, Ready for review: auth'], 'that agent at rest')
        close()

        # Too narrow for the panel, the same tab is a sheet. What was damaged is copied aside before it is replaced.
        damaged = '{"version": 1, "name": "sho'
        (folder/'project.json').write_text(damaged)
        configure('window_zoom = 1.5\n')
        launch(size='640x400')
        # A new window takes the desktop's keyboard: what was typed is checked before it is run, and asked again.
        for _ in range(4):
            call('key', 'p', '--ctrl', '--shift')
            time.sleep(.3)
            call('key', 'a', '--ctrl'); call('text', 'Show project')
            time.sleep(.3)
            typed = next((node.get('value') for node in nodes() if node.get('label') == 'Command search'), None)
            if typed == 'Show project':
                call('key', 'Enter')
                break
            call('key', 'Escape')
            time.sleep(.3)
        wait(lambda: 'Close project' in labels() and 'Message the lead' in labels(), 'the sheet of the project')
        wait(says('Sort the NECTARINE crates', 'What this project had saved about its lead and agents was damaged'), 'the chat and the notice of the damage')
        copies = list(folder.glob('project-recovery-*.json'))
        assert len(copies) == 1 and copies[0].read_text() == damaged and stat.S_IMODE(copies[0].stat().st_mode) == 0o600, copies
        # Its strip of agents is one row there, their list is away, and the chat keeps its lines over the composer.
        wait(agents_control, 'the strip of the agents')
        assert roster(2) and not listed(2) and 'Project details' in labels() and 'Project menu' in labels(), labels()
        assert says('AGENT-DID: Refactor the PLUM module')(), 'the sheet shows little of the chat'
        shot('sheet')
        writable = flags('Message the lead')
        press_agents_control()
        wait(lambda: listed(2), 'the list of the agents over the chat')
        shot('sheet-roster-open')
        press_agents_control()
        wait(lambda: not listed(2), 'the list put away again')
        # The same sheet with more room below it: sizes are asked for in points.
        call('resize', 427, 480)
        shot('sheet-tall')
        assert 'Close project' in labels()
        send('Carry on')
        wait(lambda: noted('turn', words='Carry on'), 'a turn from the sheet')
        wait(lambda: json.loads((folder/'project.json').read_text())['version'] == 1, 'the state to be written anew')
        call('key', 'Escape')
        wait(lambda: 'Close project' not in labels() and 'Message the lead' not in labels(), 'the sheet to close')
        # Opened again and the window made wide, the sheet is the tab, with what was being written.
        call('key', 'p', '--ctrl', '--shift')
        time.sleep(.3)
        call('text', 'Show project')
        time.sleep(.3)
        call('key', 'Enter')
        wait(lambda: 'Close project' in labels(), 'the sheet again')
        write('Message the lead', 'half a thought')
        call('resize', 700, 480)
        wait(lambda: 'Close project' not in labels() and 'Message the lead' in labels() and 'Project details' in labels(), 'the tab in place of the sheet')
        assert 'half a thought' in shown(), 'what was being written was lost with the sheet'
        close()
        assert len(list(folder.glob('project-recovery-*.json'))) == 1 and copies[0].read_text() == damaged

        # What a later Neptune saved is shown and never written.
        later = json.loads((folder/'project.json').read_text())
        later['version'] = 2
        (folder/'project.json').write_text(json.dumps(later))
        kept = {path.name: path.read_bytes() for path in folder.iterdir() if path.is_file()}
        leads = len(noted('lead'))
        configure()
        launch()
        panel()
        wait(says('This project is read-only', 'It was saved by a newer Neptune', 'Sort the NECTARINE crates'), 'the read-only project')
        assert flags('Message the lead') != writable, 'a read-only project takes a message'
        capture('read-only')
        segment('Watches')
        wait(says('Watches are off while this project is read-only.'), 'the read-only watches')
        assert 'Agent updates' not in shown() and 'Always on' not in shown() and 'Add watch' not in labels(), shown()
        shot('read-only-watches')
        segment('Context')
        shot('read-only-context')
        close()
        assert {path.name: path.read_bytes() for path in folder.iterdir() if path.is_file()} == kept, 'a read-only project was written'
        assert len(noted('lead')) == leads, 'a read-only project started its lead'
        private('NECTARINE', 'OLIVE', 'Weekly', 'LEAD-')
        checks.append('too narrow for the panel the project is a sheet; a damaged project.json is copied aside before it is replaced; one saved by a later Neptune is shown, starts no lead and is not written')
        (output/'result.json').write_text(json.dumps({'status': 'passed', 'platform': sys.platform, 'features': ['inspection'], 'provider': 'deterministic fixture and a stand-in gh', 'idle_frames_a_second': idle, 'checks': checks, 'address': endpoint}, indent=2))
        print(output)
    except Exception:
        if process and process.poll() is None:
            try:
                call('screenshot', output/'failure.png')
                (output/'failure.txt').write_text(shown())
            except (OSError, RuntimeError, subprocess.TimeoutExpired): pass
        print(output)
        raise
    finally:
        if process: H['stop_owned'](process)
        log.close()

if __name__ == '__main__': main()
