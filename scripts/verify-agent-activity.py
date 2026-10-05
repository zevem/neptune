#!/usr/bin/env python3
"""Focused native proof that the agents tab follows what CLI agents report.

Deterministic `claude` and `codex` fixtures run in an isolated instance, read
the hooks Neptune injects for them and fire those hooks, or set the terminal
title, on request. `opencode` and `pi` fixtures load the plugin and extension
Neptune names for them (they need `node`) and hand them events; a `gemini`
fixture only sets its title. A second instance opens an SSH workspace through
a stand-in `ssh` that runs the host's command here, with a home of its own,
so the adapters installed there report through the terminal. The rows of the
agents tab are read back through inspection.
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
    settings = args[args.index('--settings')+1]
    # On an SSH host the settings are a file the adapters installed.
    settings = json.loads(Path(settings).read_text() if settings.startswith('/') else settings)
    hooks = {event: groups[0]['hooks'][0] for event, groups in settings['hooks'].items()}
elif provider == 'gemini':
    hooks = {}
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
if provider != 'gemini': fire('SessionStart', source='startup')
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
    elif words[0] == 'forge':
        # What a printed file or a replayed log could carry: no credential.
        sys.stdout.write('\033]7717;neptune;guess;run;open;codex\007'); sys.stdout.flush()
'''

# Loads what Neptune added to the launch as the real CLI would, and hands it
# the events typed at it: `emit TYPE [JSON properties]`.
PLUGIN_FIXTURE = r'''#!/usr/bin/env node
const fs = require('fs'), path = require('path'), readline = require('readline');
const provider = path.basename(process.argv[1]);
const args = process.argv.slice(2);
(async () => {
  let emit;
  if (provider === 'opencode') {
    const plugins = JSON.parse(process.env.OPENCODE_CONFIG_CONTENT).plugin;
    const hooks = await (await import(plugins[plugins.length - 1])).Neptune({directory: process.cwd()});
    emit = (type, properties) => hooks.event({event: {type, properties}});
  } else {
    const handlers = {};
    (await import(args[args.indexOf('-e') + 1])).default({on: (name, handler) => { handlers[name] = handler; }});
    const ctx = {cwd: process.cwd(), hasUI: true, sessionManager: {getSessionId: () => '01a10a88-4d43-71a3-8741-4822706a387c'}};
    // `sub` in the properties stands for a subagent's session, which has no terminal.
    emit = (type, properties) => handlers[type] && handlers[type]({private: 'PRIVATE PROMPT', ...properties}, {...ctx, hasUI: !properties.sub});
  }
  fs.appendFileSync(process.env.NEPTUNE_AGENT_TEST_LOG, JSON.stringify({provider, args}) + '\n');
  console.log(provider.toUpperCase() + ' READY');
  for await (const typed of readline.createInterface({input: process.stdin})) {
    const line = typed.replace(/\x1b/g, '').trim();
    const [word, type, ...rest] = line.split(/\s+/);
    if (word === 'exit') break;
    // The real CLI repeats "busy" through a turn, the last time just before "idle".
    if (word === 'burst') { for (const state of ['busy', 'busy', 'busy', 'idle']) emit('session.status', {sessionID: type, status: {type: state}}); console.log('BURST'); }
    if (word === 'emit') { await emit(type, rest.length ? JSON.parse(rest.join(' ')) : {}); console.log('EMITTED ' + type); }
    if (word === 'title') process.stdout.write('\x1b]0;' + line.slice(6) + '\x07');
  }
  process.exit(0);
})();
'''
# Stands in for the OpenSSH client: the host is this machine with another home.
SSH_FIXTURE = '''#!/bin/sh
while [ "$1" != -- ]; do shift; done
HOME=$NEPTUNE_TEST_REMOTE_HOME SHELL=/bin/bash exec /bin/sh -c "$3"
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
    for provider in ('codex', 'claude', 'gemini'):
        path = fixtures/provider; path.write_text(FIXTURE); path.chmod(0o700)
    # `pi` is taken for the agent only where it resolves into its package.
    package = output / 'node_modules' / 'pi-coding-agent'; package.mkdir(parents=True)
    (package/'pi').write_text(PLUGIN_FIXTURE); (package/'pi').chmod(0o700)
    (fixtures/'pi').symlink_to(package/'pi')
    for provider in ('opencode', 'omp'):
        (fixtures/provider).write_text(PLUGIN_FIXTURE); (fixtures/provider).chmod(0o700)
    remote_home = output / 'remote-home'; remote_home.mkdir()
    client = output / 'client'; client.mkdir()
    (client/'ssh').write_text(SSH_FIXTURE); (client/'ssh').chmod(0o700)
    (data/'config.toml').write_text('shell = "/bin/bash"\nconfirm_close = false\nwarn_running_processes = false\n')
    (home/'.bashrc').write_text(f'export PATH={shlex.quote(str(fixtures))}:"$PATH"\nPS1="test $ "\n')
    endpoint = H['free_endpoint']()
    # A run started from a Neptune terminal must not reach that instance's bridge.
    env = {key: value for key, value in os.environ.items() if not key.startswith('NEPTUNE_AGENT_')}
    env.pop('WAYLAND_DISPLAY', None)
    path = [part for part in env['PATH'].split(':') if '/neptune-agents-' not in part]
    env.update(HOME=str(home), EGUI_INSPECTION=endpoint, NEPTUNE_AGENT_TEST_LOG=str(output/'events.jsonl'),
               NEPTUNE_TEST_REMOTE_HOME=str(remote_home), PATH=':'.join([str(fixtures), *path]))
    env.pop('XDG_CACHE_HOME', None)
    # The host's login files set its search path anew, as many do.
    (remote_home/'.bash_profile').write_text(f'PATH={shlex.quote(":".join([str(fixtures), *path]))}\nPS1="host $ "\n')
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

        # OpenCode and pi: the plugin and the extension Neptune named for the launch.
        session = 'ses_ef579273dffe5vpZTGF29Kt3yQ'
        busy = 'emit session.status {"sessionID":"%s","status":{"type":"busy"}}'
        idle = 'emit session.status {"sessionID":"%s","status":{"type":"idle"}}'
        text('opencode')
        fired(3)
        shows('Codex, Asked a question', 'OpenCode, Idle')
        text(busy % session)
        shows('Codex, Asked a question', 'OpenCode, Working')
        text('title OC | Fix the build')
        wait(lambda seen: any(row.endswith(': Fix the build') for row in seen), "OpenCode's session title")
        # A subagent's session turning idle is not the end of the turn.
        child = 'ses_ef579273dffe5vpZTGF29Kt3yZ'
        text('emit session.created {"sessionID":"%s","info":{"parentID":"%s"}}' % (child, session))
        text(busy % child)
        text(idle % child)
        time.sleep(.6)
        shows('Codex, Asked a question', 'OpenCode, Working')
        text('emit permission.asked {"sessionID":"%s","permission":"bash","metadata":{"command":"PRIVATE COMMAND"}}' % child)
        shows('Codex, Asked a question', 'OpenCode, Needs permission')
        text('emit permission.replied {"sessionID":"%s","reply":"once"}' % child)
        shows('Codex, Asked a question', 'OpenCode, Working')
        text('emit question.asked {"sessionID":"%s","questions":["PRIVATE QUESTION"]}' % session)
        shows('Codex, Asked a question', 'OpenCode, Asked a question')
        text('emit question.rejected {"sessionID":"%s"}' % session)
        text(idle % session)
        shows('Codex, Asked a question', 'OpenCode, Idle')
        # Reports reach Neptune in the order of their moments, however close.
        for _ in range(3):
            text('burst ' + session)
            time.sleep(.7)
            shows('Codex, Asked a question', 'OpenCode, Idle')
        # "busy" said again while a person is asked does not hide the request.
        text(busy % session)
        text('emit permission.asked {"sessionID":"%s"}' % session)
        shows('Codex, Asked a question', 'OpenCode, Needs permission')
        text(busy % session)
        time.sleep(.7)
        shows('Codex, Asked a question', 'OpenCode, Needs permission')
        text('emit permission.replied {"sessionID":"%s","reply":"reject"}' % session)
        text(idle % session)
        shows('Codex, Asked a question', 'OpenCode, Idle')
        def kept():
            saved = json.loads((data/'workspaces.json').read_text())
            return [pane['agent'] for workspace in saved['workspaces'] for pane in workspace['panes'] if pane.get('agent')]
        wait(lambda _: {'kind': 'opencode', 'session_id': session, 'cwd': str(data)} in kept(), "OpenCode's session in the saved state")
        text('exit')
        shows('Codex, Asked a question')
        text('pi')
        fired(4)
        shows('Codex, Asked a question', 'pi, Idle')
        text('emit agent_start')
        shows('Codex, Asked a question', 'pi, Working')
        text('emit ui_prompt_start')
        shows('Codex, Asked a question', 'pi, Needs input')
        text('emit ui_prompt_end')
        shows('Codex, Asked a question', 'pi, Working')
        text('emit agent_end')
        text('emit agent_settled')
        shows('Codex, Asked a question', 'pi, Idle')
        text('exit')
        shows('Codex, Asked a question')
        # Oh My Pi: the same extension, with its approvals, question tool,
        # subagents and a loop it continues by itself.
        text('omp')
        fired(5)
        shows('Codex, Asked a question', 'Oh My Pi, Idle')
        text('emit agent_start')
        shows('Codex, Asked a question', 'Oh My Pi, Working')
        text('emit agent_start {"sub":true}')
        text('emit agent_end {"sub":true}')
        text('emit agent_end {"willContinue":true}')
        time.sleep(.6)
        shows('Codex, Asked a question', 'Oh My Pi, Working')
        text('emit tool_approval_requested {"toolName":"bash","sub":true}')
        shows('Codex, Asked a question', 'Oh My Pi, Needs permission')
        text('emit tool_approval_resolved {"toolName":"bash","approved":true}')
        shows('Codex, Asked a question', 'Oh My Pi, Working')
        text('emit tool_execution_start {"toolName":"ask"}')
        shows('Codex, Asked a question', 'Oh My Pi, Asked a question')
        text('emit tool_execution_end {"toolName":"ask"}')
        shows('Codex, Asked a question', 'Oh My Pi, Working')
        text('emit agent_end')
        shows('Codex, Asked a question', 'Oh My Pi, Idle')
        # Its title carries its state too, and names the session.
        text('title π ⠋ Fix the build')
        wait(lambda seen: any(row.endswith(': Fix the build') for row in seen), "Oh My Pi's session title")
        shows('Codex, Asked a question', 'Oh My Pi, Working')
        text('title π ! Fix the build')
        shows('Codex, Asked a question', 'Oh My Pi, Needs input')
        text('title π > Fix the build')
        shows('Codex, Asked a question', 'Oh My Pi, Idle')
        text('exit')
        shows('Codex, Asked a question')
        # Gemini CLI: no hooks, its title alone.
        text('gemini')
        shows('Codex, Asked a question', 'Gemini CLI, Idle')
        text('title ◇  Ready (data)')
        time.sleep(.5)
        text('title ✦  Working… (data)')
        shows('Codex, Asked a question', 'Gemini CLI, Working')
        text('title ✋  Action Required (data)')
        shows('Codex, Asked a question', 'Gemini CLI, Needs input')
        call('screenshot', output/'more-agents.png')
        text('title ◇  Ready (data)')
        shows('Codex, Asked a question', 'Gemini CLI, Idle')
        text('exit')
        shows('Codex, Asked a question')
        launched = {entry['provider']: entry.get('args', []) for entry in events()}
        assert launched['pi'][0] == '-e' and launched['pi'][1].endswith('/plugins/pi.js'), launched['pi']
        checks.append('opencode by its plugin (turns, a subagent, permission, question, session saved), pi and oh my pi by their extension, gemini by its title')

        for provider in events():
            if provider['provider'] not in ('claude', 'codex'): continue
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

        # An SSH workspace: the adapters are installed on the host and report
        # through the terminal, with no listener between the two.
        data = output / 'data-ssh'; data.mkdir()
        (data/'config.toml').write_text('confirm_close = false\nwarn_running_processes = false\n')
        remote_env = dict(env, PATH=':'.join([str(client), env['PATH']]))
        process = subprocess.Popen([str(APP), '--data-root', str(data), '--size', '1100x700', '--ssh', 'devbox', '--no-restore'], env=remote_env, stdout=log, stderr=log)
        H['wait_ready'](process, CLIENT, endpoint, 30)
        click('Toggle right panel')
        time.sleep(.4)
        click('Agents')
        shows()
        before = len(events())
        text('claude')
        fired(before + 1)
        shows('Claude Code, Idle')
        text('fire UserPromptSubmit')
        shows('Claude Code, Working')
        text('fire PermissionRequest Bash')
        shows('Claude Code, Needs permission')
        text('fire PostToolUse Bash size=6000000')
        shows('Claude Code, Working')
        text('fire PermissionRequest Bash permission_mode=bypassPermissions')
        time.sleep(.8)
        shows('Claude Code, Working')
        text('fire PreToolUse AskUserQuestion')
        shows('Claude Code, Asked a question')
        call('screenshot', output/'ssh.png')
        call('resize', 640, 440)
        time.sleep(.5)
        call('screenshot', output/'ssh-narrow.png')
        call('resize', 1100, 700)
        time.sleep(.5)
        text('fire PostToolUse AskUserQuestion')
        text('fire Stop')
        shows('Claude Code, Idle')
        # Output that looks like a report but lacks the credential is only output.
        text('forge')
        time.sleep(.6)
        shows('Claude Code, Idle')
        text('exit')
        shows()
        text('codex')
        fired(before + 2)
        shows('Codex, Idle')
        text('fire UserPromptSubmit')
        shows('Codex, Working')
        text('fire Interrupt')
        shows('Codex, Idle')
        text('exit')
        shows()
        text('pi')
        fired(before + 3)
        shows('pi, Idle')
        text('emit agent_start')
        shows('pi, Working')
        text('emit agent_end')
        shows('pi, Idle')
        text('exit')
        shows()
        text('gemini')
        shows('Gemini CLI, Idle')
        text('title ✋  Action Required (data)')
        shows('Gemini CLI, Needs input')
        text('exit')
        shows()
        installed = remote_home/'.cache/neptune/agents'
        assert (installed.stat().st_mode & 0o777) == 0o700, 'the adapters are private to the user'
        assert sorted(path.name for path in (installed/'bin').iterdir()) == ['claude', 'codex', 'gemini', 'omp', 'opencode', 'pi']
        saved = (data/'workspaces.json').read_text()
        assert 'PRIVATE' not in saved and '"agent":{' not in saved.replace(' ', ''), 'an agent on a host is not saved'
        checks.append('ssh workspace: claude, codex, pi and gemini on the host are listed through the terminal; a line without the credential is not; nothing is saved')
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
