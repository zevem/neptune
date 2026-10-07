#!/usr/bin/env python3
"""Focused native agent restoration proof using isolated, deterministic CLI fixtures.
Real provider hook behavior requires a separate installed-CLI smoke test.
"""
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
import json, os, shlex, signal, subprocess, sys, tomllib, uuid
from pathlib import Path
args = sys.argv[1:]
provider = Path(sys.argv[0]).name
if '--help' in args:
    print('--no-daemon'); sys.exit(0)
if provider == 'claude':
    hook = json.loads(args[args.index('--settings')+1])['hooks']['SessionStart'][0]['hooks'][0]['command']
    flag = '--resume'
else:
    hook = tomllib.loads(args[args.index('-c')+1])['hooks']['SessionStart'][0]['hooks'][0]['command']
    flag = 'resume'
def link(url):
    # Starts Neptune's tool server the way each provider is told to, and calls its tool.
    if provider == 'claude':
        server = json.loads(args[args.index('--mcp-config')+1])['mcpServers']['neptune']
        env = os.environ
    else:
        server = {}
        for index, arg in enumerate(args[:-1]):
            if arg == '-c' and args[index+1].startswith('mcp_servers.neptune.'):
                key, value = args[index+1].removeprefix('mcp_servers.neptune.').split('=', 1)
                server[key] = tomllib.loads('value=' + value)['value']
        # Codex gives a tool server only the environment its configuration names.
        env = {k: v for k, v in os.environ.items() if not k.startswith('NEPTUNE_AGENT_')}
        env.update({k.removeprefix('env.'): v for k, v in server.items() if k.startswith('env.')})
    messages = [{'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-06-18'}},
                {'jsonrpc':'2.0','method':'notifications/initialized'},
                {'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':'link_pull_request','arguments':{'url':url}}}]
    result = subprocess.run([server['command'], *server['args']], input=''.join(json.dumps(m)+'\n' for m in messages), text=True, capture_output=True, env=env, check=True)
    print('LINK: ' + json.loads(result.stdout.splitlines()[-1])['result']['content'][0]['text'], flush=True)
resumed = flag in args
session = args[args.index(flag)+1] if resumed else str(uuid.uuid4())
subprocess.run(hook, shell=True, input=json.dumps({'session_id':session,'cwd':os.getcwd(),'hook_event_name':'SessionStart'}), text=True, check=True)
with open(os.environ['NEPTUNE_AGENT_TEST_LOG'], 'a') as log: log.write(json.dumps({'provider':provider,'session':session,'resumed':resumed})+'\n')
print('\033[2J\033[H' + provider.upper() + (' RESUMED ' if resumed else ' SESSION ') + session, flush=True)
def interrupt(*_):
    with open(os.environ['NEPTUNE_AGENT_TEST_LOG'], 'a') as log: log.write(json.dumps({'interrupt':provider})+'\n')
    print('INTERRUPT HANDLED', flush=True)
signal.signal(signal.SIGINT, interrupt)
while True:
    line = sys.stdin.readline()
    if not line or line.strip() == 'exit': break
    if line.startswith('link '): link(line.split()[1]); continue
    print('AGENT INPUT: ' + line.strip(), flush=True)
'''

def main():
    (ROOT / 'artifacts').mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='agent-restore-', dir=ROOT/'artifacts')).resolve()
    data = output / 'data'; data.mkdir()
    home = output / 'home'; home.mkdir()
    fixtures = output / 'bin'; fixtures.mkdir()
    for provider in ('codex', 'claude'):
        path = fixtures/provider; path.write_text(FIXTURE); path.chmod(0o700)
    # Stands in for the desktop's browser launcher and records what it is asked to open.
    opener = fixtures/'xdg-open'
    opener.write_text(f'#!/bin/sh\necho "$1" >> {shlex.quote(str(output/"opened.log"))}\n'); opener.chmod(0o700)
    # Pull request status is another script's subject; this one stays off the network.
    gh = fixtures/'gh'; gh.write_text('#!/bin/sh\nexit 1\n'); gh.chmod(0o700)
    (data/'config.toml').write_text('shell = "/bin/bash"\nconfirm_close = false\nwarn_running_processes = false\n')
    (home/'.bashrc').write_text(f'export PATH={shlex.quote(str(fixtures))}:"$PATH"\nPS1="test $ "\n')
    endpoint = H['free_endpoint']()
    env = os.environ.copy(); env.pop('WAYLAND_DISPLAY',None)
    env.update(HOME=str(home), EGUI_INSPECTION=endpoint, NEPTUNE_AGENT_TEST_LOG=str(output/'events.jsonl'))
    env['PATH'] = str(fixtures) + ':' + env['PATH']
    process = None
    log = (output/'app.log').open('w')
    def call(*args): return H['inspect'](CLIENT, endpoint, *map(str,args))
    def wait(predicate):
        end=time.monotonic()+15
        while time.monotonic()<end:
            try:
                value=predicate()
                if value: return value
            except (OSError, ValueError, KeyError): pass
            time.sleep(.05)
        raise AssertionError('Timed out waiting for native agent state')
    def saved(): return json.loads((data/'workspaces.json').read_text())
    def agents(): return [p.get('agent') for w in saved()['workspaces'] for p in w['panes']]
    def links(): return [p.get('pull_requests', []) for w in saved()['workspaces'] for p in w['panes']]
    def text(value): call('text',value); call('key','Enter')
    def launch(restore):
        nonlocal process
        args=[str(APP),'--data-root',str(data),'--size','1100x700']
        if not restore: args += ['--cwd', str(data), '--no-restore']
        process=subprocess.Popen(args,env=env,stdout=log,stderr=log)
        H['wait_ready'](process,CLIENT,endpoint,30)
    def click(label):
        nodes=call('tree')['Tree']['accesskit']['nodes']
        button=next(n for _,n in nodes if n['properties'].get('label')==label)
        bounds=button['properties']['bounds']
        call('click',(bounds['x0']+bounds['x1'])/2,(bounds['y0']+bounds['y1'])/2)
    def close():
        click('Close window')
        process.wait(timeout=10)
    def opens(number, label='Open pull request zevem/neptune#{}'):
        # The browser handoff uses xdg-open only on Linux and the BSDs.
        if not sys.platform.startswith(('linux','freebsd')): return
        click(label.format(number))
        wait(lambda: (output/'opened.log').read_text().splitlines()[-1].endswith(f'/pull/{number}'))
    try:
        launch(False)
        call('screenshot',output/'before.png')
        text('claude')
        wait(lambda: len(agents())==1 and agents()[0] and agents()[0]['session_id'])
        pull = 'https://github.com/zevem/neptune/pull/'
        text(f'link {pull}83')
        wait(lambda: links()==[[pull+'83']])
        # A terminal alone in view has no tab; the toolbar carries its link.
        call('screenshot',output/'linked-alone.png')
        opens(83)
        call('key','d',*(['--cmd'] if sys.platform == 'darwin' else ['--ctrl','--shift']))
        wait(lambda: len(agents())==2)
        text('codex')
        wait(lambda: all(a and a['session_id'] for a in agents()))
        original=agents()
        assert original[0]['session_id'] != original[1]['session_id']
        # Each terminal keeps its own links; a repeat and a non-address add nothing.
        for target in (f'{pull}84/files', f'{pull}84', 'https://github.com/zevem/neptune', f'{pull}85'):
            text(f'link {target}')
        linked=[[pull+'83'],[pull+'84',pull+'85']]
        wait(lambda: links()==linked)
        opens(84)
        # More than two share one number that lists them all.
        text(f'link {pull}86')
        linked[1].append(pull+'86')
        wait(lambda: links()==linked)
        wait(lambda: click('Pull requests') or True)
        wait(lambda: any(n['properties'].get('label')=='zevem/neptune#85' for _,n in call('tree')['Tree']['accesskit']['nodes']))
        time.sleep(.5) # The menu fades in.
        call('screenshot',output/'menu.png')
        opens(85, 'zevem/neptune#{}')
        call('screenshot',output/'running.png')
        close()
        assert agents()==original and links()==linked
        launch(True)
        wait(lambda: agents()==original and links()==linked)
        # Wait for visible terminal output, not just the loaded saved references.
        def resumed_text():
            events=[json.loads(line) for line in (output/'events.jsonl').read_text().splitlines()]
            return len([event for event in events if event.get('resumed')]) >= 2
        wait(resumed_text)
        call('screenshot',output/'restored.png')
        call('resize',640,440)
        call('screenshot',output/'restored-narrow.png')
        call('key','c','--ctrl')
        wait(lambda: 'interrupt' in (output/'events.jsonl').read_text())
        assert agents()==original
        text('exit')
        wait(lambda: agents()[1] is None and links()==[linked[0],[]])
        close()
        launch(True)
        wait(lambda: sum(json.loads(line).get('resumed',False) for line in (output/'events.jsonl').read_text().splitlines()) >= 3)
        assert agents()[1] is None
        call('screenshot',output/'agent-exited.png')
        close()
        (output/'result.json').write_text(json.dumps({'status':'passed','platform':sys.platform,'features':['inspection'],'provider':'deterministic fixtures','checks':['two providers in same directory','exact IDs after graceful close/reopen','Ctrl+C preserves agent ownership','normal agent exit returns to shell','exited agent remains shell after reopen','pull requests linked through each provider\'s tool server stay with their terminal across reopen and leave with their agent','a linked number opens its pull request from the toolbar, a tab and the menu of a terminal with several'],'address':endpoint},indent=2))
        print(output)
    except Exception:
        if process and process.poll() is None:
            call('screenshot',output/'failure.png')
        print(output)
        raise
    finally:
        if process: H['stop_owned'](process)
        log.close()

if __name__=='__main__': main()
