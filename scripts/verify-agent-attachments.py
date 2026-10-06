#!/usr/bin/env python3
"""Focused native proof of the files an agent attaches to its terminal, using
isolated, deterministic CLI fixtures that call Neptune's tool server the way
each provider is configured to. Whether an installed CLI follows the tool's
instructions needs a separate smoke test.
"""
import json
import os
from pathlib import Path
import runpy
import shlex
import struct
import subprocess
import sys
import tempfile
import time
import zlib

ROOT = Path(__file__).resolve().parents[1]
H = runpy.run_path(str(ROOT / 'scripts/native-harness.py'))
APP = ROOT / 'target/debug/neptune'
CLIENT = ROOT / 'target/debug/neptune-inspect'
FIXTURE = r'''#!/usr/bin/env python3
import json, os, subprocess, sys, tomllib, uuid
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
def attach(path, title):
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
    arguments = {'path': path, **({'title': title} if title else {})}
    messages = [{'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-06-18'}},
                {'jsonrpc':'2.0','method':'notifications/initialized'},
                {'jsonrpc':'2.0','id':2,'method':'tools/list'},
                {'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'attach_file','arguments':arguments}}]
    result = subprocess.run([server['command'], *server['args']], input=''.join(json.dumps(m)+'\n' for m in messages), text=True, capture_output=True, env=env, check=True)
    answers = [json.loads(line) for line in result.stdout.splitlines()]
    assert 'attach_file' in answers[0]['result']['instructions']
    assert any(tool['name'] == 'attach_file' for tool in answers[1]['result']['tools'])
    answer = answers[2]['result']
    with open(os.environ['NEPTUNE_AGENT_TEST_LOG'], 'a') as log:
        log.write(json.dumps({'provider':provider,'path':path,'error':answer['isError'],'text':answer['content'][0]['text']})+'\n')
    print('ATTACH: ' + answer['content'][0]['text'], flush=True)
resumed = flag in args
session = args[args.index(flag)+1] if resumed else str(uuid.uuid4())
subprocess.run(hook, shell=True, input=json.dumps({'session_id':session,'cwd':os.getcwd(),'hook_event_name':'SessionStart'}), text=True, check=True)
print('\033[2J\033[H' + provider.upper() + (' RESUMED ' if resumed else ' SESSION ') + session, flush=True)
while True:
    line = sys.stdin.readline()
    if not line or line.strip() == 'exit': break
    if line.startswith('attach '):
        path, _, title = line.strip().removeprefix('attach ').partition(' :: ')
        attach(path, title)
        continue
    print('AGENT INPUT: ' + line.strip(), flush=True)
'''

def png(path, width, height, pixel):
    """Writes an RGB picture from pixel(x, y) -> (r, g, b)."""
    rows = b''.join(b'\0' + bytes(c for x in range(width) for c in pixel(x, y)) for y in range(height))
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(rows, 6)) + chunk(b'IEND', b''))

def dialog(path, accent, wide=960, tall=600):
    """A stand-in for a screenshot: a window with a title bar, a sidebar and rows."""
    def pixel(x, y):
        if y < 44: return (36, 38, 46)
        if x < 220: return (244, 245, 248) if (y - 44) % 48 > 30 or x < 24 or x > 196 else (226, 229, 236)
        if 100 < y < 150 and 260 < x < 620: return accent
        if (y - 190) % 64 < 22 and y > 190 and 260 < x < wide - 80: return (228, 231, 238)
        return (252, 252, 253)
    png(path, wide, tall, pixel)

def main():
    (ROOT/'artifacts').mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='agent-attachments-', dir=ROOT/'artifacts')).resolve()
    data = output / 'data'; data.mkdir()
    home = output / 'home'; home.mkdir()
    fixtures = output / 'bin'; fixtures.mkdir()
    project = output / 'project'; (project/'shots').mkdir(parents=True)
    for provider in ('codex', 'claude'):
        path = fixtures/provider; path.write_text(FIXTURE); path.chmod(0o700)
    # Stand in for the desktop's launchers and record what they are asked for,
    # so that no file manager or viewer of the developer's opens.
    for launcher in ('xdg-open', 'dbus-send'):
        path = fixtures/launcher
        path.write_text(f'#!/bin/sh\necho "{launcher} $*" >> {shlex.quote(str(output/"opened.log"))}\n'); path.chmod(0o700)
    dialog(project/'shots/before.png', (150, 154, 164))
    dialog(project/'shots/after.png', (64, 120, 242))
    dialog(project/'shots/narrow.png', (38, 166, 91), 420, 760)
    more = [f'more-{index:02}.png' for index in range(1, 15)]
    for index, name in enumerate(more):
        dialog(project/'shots'/name, (40 + index * 15, 200 - index * 10, 120 + index * 8), 480, 300)
    (project/'report.md').write_text('# Review\n\nThe settings dialog now saves on close.\n')
    (project/'build.log').write_text('ok\n')
    (data/'config.toml').write_text('shell = "/bin/bash"\nconfirm_close = false\nwarn_running_processes = false\n')
    (home/'.bashrc').write_text(f'export PATH={shlex.quote(str(fixtures))}:"$PATH"\nPS1="test $ "\n')
    endpoint = H['free_endpoint']()
    env = os.environ.copy(); env.pop('WAYLAND_DISPLAY',None)
    # A run started from a Neptune terminal must not hand its own agent's
    # channel to the instance under test.
    for key in [key for key in env if key.startswith('NEPTUNE_AGENT_')]: env.pop(key)
    env.update(HOME=str(home), EGUI_INSPECTION=endpoint, NEPTUNE_AGENT_TEST_LOG=str(output/'events.jsonl'))
    env['PATH'] = str(fixtures) + ':' + env['PATH']
    process = None
    log = (output/'app.log').open('w')
    def call(*args): return H['inspect'](CLIENT, endpoint, *map(str,args))
    def wait(predicate, what='native attachment state'):
        end=time.monotonic()+15
        while time.monotonic()<end:
            try:
                value=predicate()
                if value: return value
            except (OSError, ValueError, KeyError, IndexError, StopIteration): pass
            time.sleep(.05)
        raise AssertionError(f'Timed out waiting for {what}')
    def saved(): return json.loads((data/'workspaces.json').read_text())
    def panes(): return [p for w in saved()['workspaces'] for p in w['panes']]
    def agents(): return [p.get('agent') for p in panes()]
    def attached(): return [[(Path(f['path']).name, f.get('title')) for f in p.get('attachments', [])] for p in panes()]
    def events(): return [json.loads(line) for line in (output/'events.jsonl').read_text().splitlines()]
    def opened(): return (output/'opened.log').read_text().splitlines() if (output/'opened.log').exists() else []
    def text(value): call('text',value); call('key','Enter')
    def nodes(): return [n['properties'] for _,n in call('tree')['Tree']['accesskit']['nodes']]
    def labelled(label):
        # In reading order, so that an index names the same control each time.
        found=[n for n in nodes() if n.get('label')==label and 'bounds' in n]
        return sorted(found, key=lambda n: (n['bounds']['y0'], n['bounds']['x0']))
    def click(label, index=0):
        bounds=labelled(label)[index]['bounds']
        call('click',(bounds['x0']+bounds['x1'])/2,(bounds['y0']+bounds['y1'])/2)
        return True
    def launch(restore):
        nonlocal process
        args=[str(APP),'--data-root',str(data),'--size','1100x700']
        if not restore: args += ['--cwd', str(project), '--no-restore']
        process=subprocess.Popen(args,env=env,stdout=log,stderr=log)
        H['wait_ready'](process,CLIENT,endpoint,30)
    def close():
        click('Close window')
        process.wait(timeout=10)
    def shot(name):
        time.sleep(.5) # Menus and the full view fade in.
        call('screenshot',output/name)
    def attach(path, title=None):
        count=len(events()) if (output/'events.jsonl').exists() else 0
        text(f'attach {path}' + (f' :: {title}' if title else ''))
        return wait(lambda: events()[count], f'the tool to answer for {path}')
    def viewing(title):
        # The full view names its picture and how to leave it.
        return any(title in (n.get('label') or '') and 'Escape' in (n.get('label') or '') for n in nodes())
    def menu(): return wait(lambda: click('Attached files'), 'the attached files control')
    linux = sys.platform.startswith(('linux','freebsd'))
    try:
        launch(False)
        text('claude')
        wait(lambda: len(agents())==1 and agents()[0] and agents()[0]['session_id'])
        shot('nothing-attached.png')
        # By absolute path, relative to the agent's directory, and with a title.
        assert not attach(project/'shots/before.png', 'Before: the settings dialog')['error']
        assert not attach('shots/after.png', 'After: it saves on close')['error']
        assert not attach('report.md')['error']
        # A folder and a missing file are refused, and say why.
        for path, why in (('shots', 'is a folder'), ('shots/gone.png', 'gone.png')):
            answer = attach(path)
            assert answer['error'] and why in answer['text'], answer
        first=[('before.png','Before: the settings dialog'),('after.png','After: it saves on close'),('report.md',None)]
        wait(lambda: attached()==[first])
        assert all(Path(f['path']).is_absolute() for f in panes()[0]['attachments'])
        # A terminal alone in view has no tab; the toolbar carries its count.
        shot('attached-alone.png')
        menu()
        wait(lambda: labelled('View attached file After: it saves on close'), 'the pictures to be read')
        assert labelled('Open attached file report.md')
        shot('menu.png')
        # A row shows its file in the file manager, and the list closes.
        click('Reveal in file manager', 0)
        if linux:
            wait(lambda: 'report.md' in opened()[-1] and 'ShowItems' in opened()[-1], 'the file manager handoff')
        wait(lambda: not labelled('Open attached file report.md'))
        # Another kind of file opens with its application.
        menu(); wait(lambda: click('Open attached file report.md'))
        if linux:
            wait(lambda: opened()[-1]==f'xdg-open {project/"report.md"}', 'the file to be opened')
        # A picture opens at full size, where its neighbours are a step away.
        menu(); wait(lambda: click('View attached file After: it saves on close'))
        wait(lambda: labelled('Next picture'), 'the full view')
        shot('view.png')
        call('key','ArrowRight')
        wait(lambda: any('Before: the settings dialog' in (n.get('label') or '') and 'Escape' in (n.get('label') or '') for n in nodes()), 'the next picture')
        shot('view-next.png')
        click('Previous picture')
        wait(lambda: any('After: it saves on close' in (n.get('label') or '') and 'Escape' in (n.get('label') or '') for n in nodes()), 'the previous picture')
        call('key','Escape')
        wait(lambda: not labelled('Next picture'))
        # All the pictures open on the newest, each a click on its small copy away.
        menu(); wait(lambda: click('View all pictures'))
        wait(lambda: viewing('After: it saves on close') and labelled('Show Before: the settings dialog'), 'the band of small copies')
        shot('view-all.png')
        click('Show Before: the settings dialog')
        wait(lambda: viewing('Before: the settings dialog'), 'the picture of the clicked copy')
        shot('view-all-other.png')
        call('key','Escape')
        wait(lambda: not labelled('Next picture'))
        # The same file again moves to the top under its new title.
        assert not attach('shots/before.png', 'Before')['error']
        first=[first[1],first[2],('before.png','Before')]
        wait(lambda: attached()==[first])
        # Removing one leaves the list open for the next.
        menu(); wait(lambda: labelled('Open attached file report.md'))
        rows=labelled('Dismiss')
        middle=rows[1]['bounds']
        call('click',(middle['x0']+middle['x1'])/2,(middle['y0']+middle['y1'])/2)
        first=[first[0],first[2]]
        wait(lambda: attached()==[first])
        assert labelled('View attached file Before')
        call('key','Escape')
        # Each terminal keeps its own files.
        call('key','d',*(['--cmd'] if sys.platform == 'darwin' else ['--ctrl','--shift']))
        wait(lambda: len(agents())==2)
        text('codex')
        wait(lambda: all(a and a['session_id'] for a in agents()))
        assert not attach('shots/narrow.png', 'The narrow layout')['error']
        assert not attach('build.log')['error']
        second=[('narrow.png','The narrow layout'),('build.log',None)]
        wait(lambda: attached()==[first,second])
        shot('split.png')
        wait(lambda: click('Attached files', 1))
        wait(lambda: labelled('View attached file The narrow layout'), 'the second terminal\'s list')
        shot('split-menu.png')
        call('key','Escape')
        # More pictures than the band holds move along with the one in view.
        for name in more: assert not attach(f'shots/{name}')['error']
        second += [(name, None) for name in more]
        wait(lambda: attached()==[first,second])
        call('resize',640,440)
        wait(lambda: click('Attached files', 1))
        wait(lambda: click('View all pictures'), 'every picture to be read')
        wait(lambda: viewing(more[-1]) and labelled(f'Show {more[-1]}'), 'the newest of many')
        assert not labelled('Show The narrow layout')
        shot('view-many-narrow.png')
        call('key','ArrowLeft')
        wait(lambda: viewing('The narrow layout') and labelled('Show The narrow layout'), 'the oldest of many')
        assert not labelled(f'Show {more[-1]}')
        shot('view-many-narrow-end.png')
        call('key','Escape')
        wait(lambda: not labelled('Next picture'))
        call('resize',1100,700)
        close()
        assert attached()==[first,second]
        # They return with their agents.
        launch(True)
        wait(lambda: attached()==[first,second] and len(labelled('Attached files'))==2)
        call('resize',640,440)
        wait(lambda: click('Attached files', 0))
        wait(lambda: labelled('View attached file Before'), 'the restored list')
        shot('restored-narrow-menu.png')
        click('View attached file Before')
        wait(lambda: labelled('Next picture'))
        shot('restored-narrow-view.png')
        call('key','Escape')
        call('resize',1100,700)
        # The whole list at once, and what is left leaves with its agent.
        wait(lambda: click('Attached files', 0))
        wait(lambda: click('Dismiss all'))
        wait(lambda: attached()==[[],second])
        wait(lambda: len(labelled('Attached files'))==1)
        text('exit')
        wait(lambda: agents()[1] is None and attached()==[[],[]])
        shot('agent-exited.png')
        close()
        (output/'result.json').write_text(json.dumps({'status':'passed','platform':sys.platform,'features':['inspection'],'provider':'deterministic fixtures','checks':['files attached through each provider\'s tool server by absolute and relative path, with and without a title','a folder and a missing file are refused with the reason','the toolbar and each tab count their own terminal\'s files','the list shows pictures and other files, reveals one in the file manager and opens another with its application','a picture opens at full size and steps to its neighbours by key and by control','all pictures open from the list on the newest and each is shown by its small copy','more small copies than fit move along with the picture in view','the same file again moves to the top under its new title','one file and all files are removed from the list','attached files return after close and reopen and leave with their agent','narrow window list and full view'],'address':endpoint},indent=2))
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
