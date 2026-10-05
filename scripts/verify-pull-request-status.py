#!/usr/bin/env python3
"""Focused native proof of the live pull request status on a linked number.

Deterministic CLI fixtures link pull requests in an isolated native app, and a
stand-in `gh` answers Neptune's request from a file this script rewrites. The
stand-in proves the request Neptune makes and how it reads the answer, not
GitHub's API or an installed GitHub CLI. Review the captures it leaves.
"""
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
H = runpy.run_path(str(ROOT / 'scripts/native-harness.py'))
FIXTURE = runpy.run_path(str(ROOT / 'scripts/verify-agent-restore.py'))['FIXTURE']
APP = ROOT / 'target/debug/neptune'
CLIENT = ROOT / 'target/debug/neptune-inspect'
GH = r'''#!/usr/bin/env python3
import json, os, re, sys
here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
query = next(a for a in args if a.startswith('query='))[6:]
asked = re.findall(r'(p\d+):repository\(owner:"([^"]+)",name:"([^"]+)"\)\{pullRequest\(number:(\d+)\)', query)
with open(os.path.join(here, 'gh.log'), 'a') as log:
    log.write(json.dumps({'args': args[:4], 'asked': [[o, r, int(n)] for _, o, r, n in asked], 'stdin': os.isatty(0)}) + '\n')
states = json.load(open(os.path.join(here, 'gh-states.json')))
if states.get('fail'): sys.exit(1)
data = {}
for alias, owner, repo, number in asked:
    pull = states.get(number)
    data[alias] = pull and {'pullRequest': {
        'state': pull['state'], 'isDraft': pull.get('draft', False),
        'commits': {'nodes': [{'commit': {'statusCheckRollup': pull.get('checks') and {'state': pull['checks']}}}]},
        'reviewThreads': {'nodes': [{'isResolved': False}] * pull.get('unresolved', 0) + [{'isResolved': True}] * 2}}}
print(json.dumps({'data': data}))
sys.exit(1 if None in data.values() else 0)
'''

def main():
    (ROOT/'artifacts').mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='pull-request-status-', dir=ROOT/'artifacts')).resolve()
    data = output / 'data'; data.mkdir()
    home = output / 'home'; home.mkdir()
    fixtures = output / 'bin'; fixtures.mkdir()
    for name, body in (('codex', FIXTURE), ('claude', FIXTURE), ('gh', GH)):
        path = fixtures/name; path.write_text(body); path.chmod(0o700)
    opener = fixtures/'xdg-open'
    opener.write_text(f'#!/bin/sh\necho "$1" >> {shlex.quote(str(output/"opened.log"))}\n'); opener.chmod(0o700)
    (home/'.bashrc').write_text(f'export PATH={shlex.quote(str(fixtures))}:"$PATH"\nPS1="test $ "\n')
    def configure(theme):
        (data/'config.toml').write_text(f'shell = "/bin/bash"\ntheme = "{theme}"\nconfirm_close = false\nwarn_running_processes = false\n')
    def states(**pulls):
        # Written whole, so the stand-in never reads half a file.
        fresh = fixtures/'gh-states.json.new'
        fresh.write_text(json.dumps({number.lstrip('n'): pull for number, pull in pulls.items()}))
        fresh.replace(fixtures/'gh-states.json')
    def asked():
        log = fixtures/'gh.log'
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    endpoint = H['free_endpoint']()
    env = os.environ.copy(); env.pop('WAYLAND_DISPLAY',None)
    env.update(HOME=str(home), EGUI_INSPECTION=endpoint, NEPTUNE_AGENT_TEST_LOG=str(output/'events.jsonl'))
    # A run started from a terminal of an installed Neptune must not answer to it.
    for name in [name for name in env if name.startswith(('NEPTUNE_AGENT_', 'CLAUDE'))]:
        if name != 'NEPTUNE_AGENT_TEST_LOG': env.pop(name)
    env['PATH'] = os.pathsep.join([str(fixtures), *(part for part in env['PATH'].split(os.pathsep) if 'neptune-agents-' not in part)])
    process = None
    log = (output/'app.log').open('w')
    def call(*args): return H['inspect'](CLIENT, endpoint, *map(str,args))
    def wait(predicate, seconds=15):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            try:
                value=predicate()
                if value: return value
            except (OSError, ValueError, KeyError, StopIteration): pass
            time.sleep(.05)
        raise AssertionError('Timed out waiting for native pull request state')
    def saved(): return json.loads((data/'workspaces.json').read_text())
    def agents(): return [p.get('agent') for w in saved()['workspaces'] for p in w['panes']]
    def links(): return [p.get('pull_requests', []) for w in saved()['workspaces'] for p in w['panes']]
    def text(value): call('text',value); call('key','Enter')
    def launch(restore, size='1100x700'):
        nonlocal process
        args=[str(APP),'--data-root',str(data),'--size',size]
        if not restore: args += ['--cwd', str(data), '--no-restore']
        process=subprocess.Popen(args,env=env,stdout=log,stderr=log)
        H['wait_ready'](process,CLIENT,endpoint,30)
    def bounds(label):
        nodes=call('tree')['Tree']['accesskit']['nodes']
        bounds=next(n for _,n in nodes if n['properties'].get('label')==label)['properties']['bounds']
        return (bounds['x0']+bounds['x1'])/2,(bounds['y0']+bounds['y1'])/2
    def click(label): call('click',*bounds(label))
    def close():
        click('Close window')
        process.wait(timeout=10)
    def shot(name):
        # A state read off the frame is drawn by the frame it asks for.
        time.sleep(.6)
        call('move', 550, 400)
        call('settle')
        call('screenshot',output/f'{name}.png')
    def read_again(seconds):
        rounds=len(asked())
        wait(lambda: len(asked())>rounds, seconds)
    pull = 'https://github.com/zevem/neptune/pull/'
    open_ = dict(state='OPEN')
    try:
        configure('graphite')
        states(n101=dict(open_, checks='PENDING'))
        launch(False)
        text('claude')
        wait(lambda: len(agents())==1 and agents()[0] and agents()[0]['session_id'])
        assert asked()==[], 'nothing is asked before a pull request is linked'
        text(f'link {pull}101')
        wait(lambda: links()==[[pull+'101']])
        wait(lambda: len(asked())==1)
        first=asked()[0]
        assert first['args']==['api','graphql','--hostname','github.com'], first
        assert first['asked']==[['zevem','neptune',101]] and not first['stdin'], first
        # A terminal alone in view has no tab; the toolbar carries its number.
        shot('alone-checks-running')
        # Running checks are read again within 15 seconds, without any input.
        states(n101=dict(open_, checks='FAILURE', unresolved=2))
        read_again(25)
        shot('alone-checks-failing-2-unresolved')
        call('move', *bounds('Open pull request zevem/neptune#101'))
        time.sleep(1.2) # The tooltip waits for the pointer to rest.
        call('screenshot',output/'alone-tooltip.png')
        # A second terminal: each has a tab, and each tab its own numbers.
        states(n101=dict(open_, checks='FAILURE', unresolved=2), n102=dict(open_, checks='SUCCESS'),
               n103=dict(state='MERGED', checks='SUCCESS', unresolved=1))
        call('key','d',*(['--cmd'] if sys.platform == 'darwin' else ['--ctrl','--shift']))
        wait(lambda: len(agents())==2)
        text('codex')
        wait(lambda: all(a and a['session_id'] for a in agents()))
        text(f'link {pull}102'); text(f'link {pull}103')
        wait(lambda: links()==[[pull+'101'],[pull+'102',pull+'103']])
        wait(lambda: any(['zevem','neptune',103] in a['asked'] for a in asked()))
        shot('tabs-failing-passing-merged')
        # One request names every pull request that is due together.
        assert all(len(a['asked'])<=3 for a in asked())
        # More than two share the newest number, with what needs a person in any.
        states(n101=dict(open_, checks='FAILURE', unresolved=2), n102=dict(open_, checks='SUCCESS'),
               n103=dict(state='MERGED', checks='SUCCESS', unresolved=1),
               n104=dict(open_, draft=True, checks='PENDING', unresolved=1), n105=dict(state='CLOSED'))
        text(f'link {pull}104'); text(f'link {pull}105'); text(f'link {pull}106')
        wait(lambda: len(links()[1])==5)
        wait(lambda: any(['zevem','neptune',106] in a['asked'] for a in asked()))
        shot('tabs-several')
        click('Pull requests')
        wait(lambda: bounds('zevem/neptune#105'))
        time.sleep(.5) # The menu fades in.
        call('screenshot',output/'menu.png')
        call('key','Escape')
        # A lookup that fails leaves a number as it was before it had a state.
        (fixtures/'gh-states.json').write_text(json.dumps({'fail': True}))
        read_again(25) # The draft's running checks are due first.
        time.sleep(1)
        shot('unavailable')
        call('resize',640,440)
        shot('narrow')
        close()
        # The states return with the links, in the Light theme.
        configure('light')
        states(n101=dict(open_, checks='FAILURE', unresolved=2), n102=dict(open_, checks='SUCCESS'),
               n103=dict(state='MERGED', checks='SUCCESS', unresolved=1),
               n104=dict(open_, draft=True, checks='PENDING', unresolved=1), n105=dict(state='CLOSED'))
        rounds=len(asked())
        launch(True)
        wait(lambda: len(asked())>rounds)
        shot('restored-light')
        close()
        (output/'result.json').write_text(json.dumps({'status':'passed','platform':sys.platform,'features':['inspection'],'provider':'deterministic fixtures and a stand-in gh','checks':['nothing is asked before a link','one gh api graphql request names the linked pull requests of a host, without a terminal on its input','running checks are read again without input and the new state is drawn','a failed lookup and a restored workspace are read again'],'requests':len(asked()),'address':endpoint},indent=2))
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
