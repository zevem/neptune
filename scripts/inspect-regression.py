#!/usr/bin/env python3
"""Verify a dedicated Neptune instance through the native inspection protocol.

Prefer scripts/native-harness.py, which owns launch, endpoint, data and cleanup.
For a manually launched instance pass its exact --addr and --data-root.
Widget lookup uses stable accessible labels. Shell command assertions currently
require a POSIX shell; OS keyboard routing is a separate Linux/X11 adapter in
scripts/native-smoke.py. No workspace state outside --data-root is accessed.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import socket
import signal
import sys
import subprocess
import time
import tomllib
repo_root = Path(__file__).resolve().parents[1]
binary_suffix = '.exe' if os.name == 'nt' else ''
parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('--client', type=Path, default=repo_root / 'target' / 'debug' / ('neptune-inspect' + binary_suffix), help='Path to the inspection client binary')
parser.add_argument('--data-root', '--config-root', dest='data_root', type=Path, required=True, help='Exact --data-root used by the app')
parser.add_argument('--addr', required=True, help='Run-owned inspection endpoint HOST:PORT')
parser.add_argument('--output', type=Path, default=repo_root / 'artifacts' / 'final-review')
parser.add_argument('--restore', action='store_true', help='Close and restart the isolated app to verify restoration')
parser.add_argument('--app', type=Path, default=repo_root / 'target' / 'debug' / ('neptune' + binary_suffix), help='Inspection-enabled app binary for --restore')
args = parser.parse_args()
if hasattr(signal, 'SIGTERM'):
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt('Regression stopped by its harness')))
app_modifier = 'cmd' if sys.platform == 'darwin' else 'ctrl+shift'
edit_modifier = 'cmd' if sys.platform == 'darwin' else 'ctrl'
args.data_root = args.data_root.resolve()
args.output = args.output.resolve()
args.client = args.client.resolve()
args.app = args.app.resolve()
args.output.mkdir(parents=True, exist_ok=True)
restored = None
restoration_log = None
report = {'address': args.addr, 'data_root': str(args.data_root), 'status': 'running', 'checks': [], 'screenshots': [], 'input': 'native egui inspection protocol', 'steps': []}

def save():
    (args.output / 'ui-regression.json').write_text(json.dumps(report, indent=2) + '\n')

def call(*arguments):
    report['steps'].append(list(map(str, arguments)))
    result = subprocess.run([str(args.client), '--addr', args.addr, *map(str, arguments)], capture_output=True, text=True, timeout=25)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    return json.loads(result.stdout)

def wait_until(description, predicate, timeout=15):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (OSError, ValueError, RuntimeError) as error:
            last_error = error
        time.sleep(0.05)
    raise RuntimeError(f'Timed out waiting for {description}; last error: {last_error}')

def endpoint_open():
    host, port = args.addr.rsplit(':', 1)
    try:
        with socket.create_connection((host, int(port)), timeout=0.15):
            return True
    except OSError:
        return False

def state():
    snapshot = json.loads((args.data_root / 'workspaces.json').read_text())
    if snapshot.get('version') != 9:
        raise RuntimeError(f'Expected workspace schema version 9, found {snapshot.get("version")}')
    return snapshot

def active_workspace(snapshot):
    return next(workspace for workspace in snapshot['workspaces'] if workspace['id'] == snapshot['active'])

def widget(label, role=None):
    return wait_until(f'widget {label!r}', lambda: find(tree(), label, role))

def key(chord):
    call('key', chord)

def tree(name=None):
    result = call('tree')
    if name:
        (args.output / f'{name}.json').write_text(json.dumps(result, indent=2) + '\n')
    accesskit = result['Tree']['accesskit']
    return [dict(node, inspection_id=ident, inspection_focused=ident == accesskit['focus']) for ident, node in accesskit['nodes']]

def center(node):
    b = node['properties']['bounds']
    return ((b['x0'] + b['x1']) / 2, (b['y0'] + b['y1']) / 2)

def click(node):
    # Dialogs can resize after their first layout pass. Settle that layout and
    # resolve the semantic target again instead of using a stale screen point.
    call('settle', 2)
    node = find(tree(), node['properties']['label'], node['role'])
    call('click', *center(node))

def find(nodes, label, role=None):
    found = [n for n in nodes if n['properties'].get('label') == label and (role is None or n['role'] == role)]
    if len(found) != 1:
        raise RuntimeError(f'Expected unique widget {label!r}; found {len(found)}')
    return found[0]

def check(name, passed, detail):
    report['checks'].append({'name': name, 'passed': bool(passed), 'detail': detail})
    save()
    if not passed:
        raise RuntimeError(f'{name}: {detail}')
    print(f'PASS {name}', flush=True)

def shot(name):
    call('settle', 60)
    result = call('screenshot', args.output / f'{name}.png')
    report['screenshots'].append(result)
    save()
    print(f'Captured {name}', flush=True)

def config():
    path = args.data_root / 'config.toml'
    return tomllib.loads(path.read_text()) if path.exists() else {}

def shell(command):
    call('text', command)
    key('Enter')
    call('settle', 60)

def bodies(nodes):
    found = [node for node in nodes if node['properties'].get('label', '').startswith('Terminal pane ')]
    labels = [node['properties']['label'] for node in found]
    if len(labels) != len(set(labels)):
        raise RuntimeError('Terminal pane accessibility identities are not unique')
    return sorted(found, key=lambda node: (node['properties']['bounds']['x0'], node['properties']['bounds']['y0']))

def verify_typed_resize():
    """Keep a real zsh input buffer through width and height changes."""
    call('resize', 1180, 760)
    panes = bodies(tree())
    click(panes[-1])
    shell("printf 'QA_RESIZE_HISTORY_KEPT\\n'")
    payload = 'QA_TYPED_BUFFER_' + '0123456789abcdef' * 19 + '0123456789'
    assert len(payload) == 330
    marker = args.data_root / 'typed-resize-output'
    marker.unlink(missing_ok=True)
    call('text', f"printf '%s' {shlex.quote(payload)} > {shlex.quote(str(marker))}")
    shot('18-typed-buffer-before-resize')
    call('resize', 900, 640)
    shot('19-typed-buffer-900x640')
    call('resize', 640, 480)
    shot('20-typed-buffer-640x480')
    call('resize', 1180, 760)
    shot('21-typed-buffer-returned')
    key('Enter')
    wait_until('typed shell buffer to execute', lambda: marker.exists() and marker.read_text() == payload)
    check('native typed buffer survives resize', marker.exists() and marker.read_text() == payload, '330-byte shell input payload survives two width/height changes and executes without corruption')
    shell("printf 'QA_TYPED_RESIZE_OK\\n'")
    call('move', 600, 747)
    shot('22-typed-buffer-executed')
try:
    key('Escape')
    call('resize', 1180, 760)
    report['info'] = call('info')
    shot('00-workspace')
    initial_nodes = tree('00-widget-tree')
    icon_labels = {'Toggle sidebar', 'Command palette', 'Minimize', 'Maximize', 'Close window', 'New workspace', 'Preferences', 'Find in terminal', 'Split right', 'Split below'}
    available_labels = {n['properties'].get('label') for n in initial_nodes if n['role'] == 'button'}
    check('icon accessibility labels', icon_labels <= available_labels, 'window, sidebar and toolbar controls must expose descriptive native button labels')
    state_path = args.data_root / 'workspaces.json'
    before_creation = wait_until('initial workspace state', state)
    key(app_modifier + '+n')
    created_state = wait_until('new workspace persistence', lambda: (saved if len(saved['workspaces']) == len(before_creation['workspaces']) + 1 else None) if (saved := state()) else None)
    created = active_workspace(created_state)
    nodes = tree('01-new-workspace-tree')
    shot('01-new-workspace')
    check('workspace creation at home', created['id'] != before_creation['active'] and Path(created['cwd']) == Path.home() and len(bodies(nodes)) == 1, 'new workspace must be selected with one terminal at home')
    check('workspace creation without a modal', not any(n['role'] == 'window' and n['properties'].get('label') == 'New workspace' for n in nodes) and not any(n['role'] == 'textInput' and n['properties'].get('label') in {'Workspace directory', 'Workspace name'} for n in nodes), 'creation must immediately return to the terminal')
    key(app_modifier + '+p')
    call('text', 'Rename workspace')
    key('Enter')
    click(widget('Workspace rename', 'textInput'))
    key(edit_modifier + '+a')
    call('text', 'Sandbox')
    shot('02-rename-workspace')
    click(widget('Save', 'button'))
    renamed_state = wait_until('workspace rename persistence', lambda: (saved if active_workspace(saved)['name'] == 'Sandbox' else None) if (saved := state()) else None)
    renamed = active_workspace(renamed_state)
    nodes = tree('02-created-workspace-tree')
    check('workspace renamed afterward', renamed['id'] == created['id'] and renamed['cwd'] == created['cwd'] and renamed['layout'] == created['layout'] and not any(n['role'] == 'window' and n['properties'].get('label') == 'Rename workspace' for n in nodes), 'rename must preserve the new workspace directory and terminal and close its sheet')
    shot('02-sandbox')
    key(app_modifier + '+d')
    call('settle', 60)
    shot('03-split-right')
    key(app_modifier + '+e')
    call('settle', 60)
    shot('04-split-below')
    panes = wait_until('three terminal panes', lambda: (panes if len(panes) == 3 else None) if (panes := bodies(tree())) else None)
    tree('04-panes-tree')
    check('three native terminal panes', len(panes) == 3, 'tree must contain three distinct interactive terminal bodies')
    for i, pane in enumerate(panes, 1):
        click(pane)
        pid_path = args.data_root / f'pane-{i}.pid'
        pid_path.unlink(missing_ok=True)
        shell(f"printf 'QA_PANE_{i}_OK\\n'; printf '%s' \"$$\" > {shlex.quote(str(pid_path))}")
    wait_until('three independent shell PID markers', lambda: all((args.data_root / f'pane-{i}.pid').exists() for i in range(1,4)))
    shell_pids = [(args.data_root / f'pane-{i}.pid').read_text() for i in range(1, 4)]
    check('three independent shell processes', len(set(shell_pids)) == 3 and all(pid.isdecimal() for pid in shell_pids), 'each focused native pane must execute commands in a different real shell')
    report['shell_pids'] = shell_pids
    shot('05-independent-shells')
    pane_labels = {n['properties'].get('label') for n in tree() if n['role'] == 'button'}
    check('pane header controls', {'Close terminal', 'Zoom terminal'} <= pane_labels, 'split panes must expose their own close and zoom controls')
    # The focused terminal owns Tab; the toolkit must not move focus into the chrome.
    tab_marker = args.data_root / 'tab-ok'
    tab_marker.unlink(missing_ok=True)
    shell(f"cat > {shlex.quote(str(tab_marker))}")
    call('text', 'a')
    key('Tab')
    call('text', 'b')
    key('Enter')
    key('ctrl+d')
    wait_until('Tab to reach the shell', lambda: tab_marker.exists() and tab_marker.read_text() == 'a\tb\n')
    focused = [n for n in tree() if n['inspection_focused']]
    check('Tab reaches the shell', tab_marker.read_text() == 'a\tb\n' and len(focused) == 1 and focused[0]['properties'].get('label', '').startswith('Terminal pane '), 'a literal Tab is delivered to the foreground process and the terminal keeps keyboard focus')
    call('context', *center(panes[-1]))
    call('settle', 60)
    menu_labels = {n['properties'].get('label') for n in tree('05-context-menu-tree') if n['role'] == 'button'}
    check('terminal context menu', {'Copy', 'Paste', 'Clear scrollback', 'Restart terminal'} <= menu_labels, 'secondary click opens the terminal menu with its actions')
    shot('05-context-menu')
    key('Escape')
    call('settle', 60)
    check('context menu dismissed', 'Restart terminal' not in {n['properties'].get('label') for n in tree()} and len(bodies(tree())) == 3, 'Escape closes the menu without changing the panes')
    edge_x, edge_y = center(find(tree(), 'Resize sidebar'))
    call('drag', edge_x, edge_y, edge_x + 60, edge_y)
    wait_until('sidebar width persistence', lambda: abs(config().get('sidebar_width', 0) - (edge_x + 60)) <= 3)
    check('sidebar resizes from its edge', abs(config().get('sidebar_width', 0) - (edge_x + 60)) <= 3 and len(bodies(tree())) == 3, 'dragging the sidebar edge saves the new width on release and keeps every pane')
    shot('05-sidebar-resized')
    call('double-click', *center(find(tree(), 'Resize sidebar')))
    wait_until('sidebar width reset', lambda: config().get('sidebar_width') == 216.0)
    check('sidebar width resets on double-click', config().get('sidebar_width') == 216.0, 'double-clicking the edge restores the default width')
    marker = args.data_root / 'interrupt-ok'
    marker.unlink(missing_ok=True)
    started = args.data_root / 'interrupt-started'
    started.unlink(missing_ok=True)
    shell(f"printf ready > {shlex.quote(str(started))}; sleep 30")
    wait_until('long-running command start', started.exists)
    key('ctrl+c')
    shell(f"printf 'QA_INTERRUPT_OK\\n' > {shlex.quote(str(marker))}")
    wait_until('interrupted shell to execute follow-up command', marker.exists, timeout=10)
    check('Ctrl+C interrupts process', marker.exists(), 'follow-up shell command executes before 30-second sleep completes')
    shell("printf 'QA_INTERRUPT_OK\\n'")
    shot('06-process-interrupted')
    key(edit_modifier + '+comma')
    nodes = tree('07-preferences-tree')
    shot('07-preferences-graphite')
    preferences = find(nodes, 'Preferences', 'window')['properties']['bounds']
    check('Preferences content viewport', preferences['y1'] - preferences['y0'] > 300, 'settings dialog must expose controls rather than collapse its scroll area')
    click(find(nodes, 'Appearance', 'radioButton'))
    call('settle', 4)
    click(find(tree(), 'Browse themes…'))
    call('settle', 4)
    click(find(tree(), 'Search themes', 'textInput'))
    call('text', 'Light')
    call('settle', 4)
    click(find(tree(), 'Light theme', 'radioButton'))
    wait_until('Light preference persistence', lambda: config().get('theme') == 'light')
    check('Light theme selected', config().get('theme') == 'light', 'theme saved to isolated native configuration')
    shot('08-preferences-light')
    click(find(tree(), 'Close preferences'))
    shot('09-light-workspace')
    key(edit_modifier + '+comma')
    click(find(tree(), 'Appearance', 'radioButton'))
    call('settle', 4)
    click(find(tree(), 'Browse themes…'))
    call('settle', 4)
    click(find(tree(), 'Graphite theme', 'radioButton'))
    wait_until('Graphite preference persistence', lambda: config().get('theme') == 'graphite')
    check('Graphite theme restored', config().get('theme') == 'graphite', 'restore theme before remaining screenshot states')
    click(find(tree(), 'Close preferences'))
    call('resize', 900, 640)
    call('settle', 60)
    shot('10-responsive-900x640')
    call('resize', 640, 480)
    call('settle', 60)
    shot('11-responsive-640x480')
    key(edit_modifier + '+comma')
    shot('12-preferences-640x480')
    key('Escape')
    call('resize', 1180, 760)
    call('settle', 60)
    before_search = state()
    key(app_modifier + '+f')
    nodes = tree('13-search-focus-tree')
    check('search receives keyboard focus after resize', any(n['role'] == 'textInput' and n['properties'].get('label') == 'Terminal search' and n['inspection_focused'] for n in nodes), 'the native focused widget must be the search field after sidebar hide/show')
    call('text', 'QA_INTERRUPT_OK')
    nodes = tree('13-search-value-tree')
    check('scrollback search field', any((n['role'] == 'textInput' and n['properties'].get('value') == 'QA_INTERRUPT_OK' for n in nodes)), 'native search field contains expected query before Enter')
    key('Enter')
    nodes = tree('13-search-tree')
    after_search = state()
    check('search preserves selected workspace', len(after_search['workspaces']) == len(before_search['workspaces']) and after_search['active'] == before_search['active'] and len(bodies(nodes)) == 3, 'Enter in search must keep the three-pane Sandbox selected and create no workspace')
    shot('13-scrollback-search')
    key('Escape')
    key(app_modifier + '+w')
    # Close-time process checks finish asynchronously. Wait for the actual
    # confirmation, not a fixed number of frames or the hidden checking phase.
    widget('Close terminal?', 'window')
    nodes = tree('14-close-tree')
    check('close confirmation shown', any((n['role'] == 'window' and n['properties'].get('label') == 'Close terminal?' for n in nodes)), 'real native confirmation dialog appears')
    shot('14-close-confirmation')
    click(find(nodes, 'Cancel', 'button'))
    nodes = tree('15-cancelled-tree')
    check('close cancelled', not any((n['role'] == 'window' and n['properties'].get('label') == 'Close terminal?' for n in nodes)) and len(bodies(nodes)) == 3, 'dialog dismissed and all terminal panes remain')
    shot('15-close-cancelled')
    if args.restore:
        click(find(nodes, 'Close window', 'button'))
        widget('Quit Neptune?', 'window')
        close_nodes = tree('16-app-close-tree')
        shot('16-app-close-confirmation')
        try:
            click(find(close_nodes, 'Quit', 'button'))
        except RuntimeError as error:
            report['close_response'] = str(error)
        wait_until('graceful inspector shutdown', lambda: not endpoint_open())
        check('graceful application close', True, 'native inspector endpoint closes after confirming app exit')
        saved = state()
        report['saved_state_after_close'] = saved
        environment = os.environ.copy()
        environment.pop('WAYLAND_DISPLAY', None)
        environment['EGUI_INSPECTION'] = args.addr
        restoration_log = open(args.output / 'restoration.log', 'w')
        restored = subprocess.Popen([str(args.app), '--data-root', str(args.data_root), '--size', '1180x760'], env=environment, stdout=restoration_log, stderr=subprocess.STDOUT, start_new_session=True)
        report['restored_pid'] = restored.pid
        def restoration_ready():
            if restored.poll() is not None:
                raise RuntimeError(f'Restoration app exited with {restored.returncode}')
            info = call('info')
            nodes = tree()
            if len(bodies(nodes)) != 3:
                return None
            report['restored_info'] = info
            return nodes
        wait_until('restored application and three terminal panes', restoration_ready, timeout=20)
        check('restoration process started', True, 'fresh native app serves inspection on the run-owned endpoint')
        restored_nodes = tree('17-restored-tree')
        check('workspace and split layout restored', any((n['role'] == 'button' and n['properties'].get('label') == 'Sandbox' and (n['properties'].get('toggled') == 'true') for n in restored_nodes)) and len(bodies(restored_nodes)) == 3, 'Sandbox restored as selected workspace with three fresh shell panes')
        call('move', 600, 747)
        shot('17-restored-workspace')
    verify_typed_resize()
    report['status'] = 'passed'
except (Exception, KeyboardInterrupt) as e:
    report['status'] = 'failed'
    report['error'] = str(e)
    try:
        shot('failure')
        tree('failure-tree')
    except Exception:
        pass
    raise
finally:
    save()
    if restored is not None and restored.poll() is None:
        restored.terminate()
        try:
            restored.wait(timeout=5)
        except subprocess.TimeoutExpired:
            restored.kill()
            restored.wait(timeout=5)
    if restoration_log is not None:
        restoration_log.close()
