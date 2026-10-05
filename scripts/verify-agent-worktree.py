#!/usr/bin/env python3
"""Focused native proof of "New agent in worktree", with deterministic CLI fixtures
in an isolated app and a repository of its own. Real provider CLIs need a
separate installed-CLI check.
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
FIXTURE = runpy.run_path(str(ROOT / 'scripts/verify-agent-restore.py'))['FIXTURE']
APP = ROOT / 'target/debug/neptune'
CLIENT = ROOT / 'target/debug/neptune-inspect'
COMMAND = ['--cmd'] if sys.platform == 'darwin' else ['--ctrl', '--shift']

def main():
    (ROOT / 'artifacts').mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='agent-worktree-', dir=ROOT / 'artifacts')).resolve()
    data = output / 'data'; data.mkdir()
    home = output / 'home'; home.mkdir()
    fixtures = output / 'bin'; fixtures.mkdir()
    repo = output / 'project'; repo.mkdir()
    trees = output / 'project.worktrees'
    for provider in ('codex', 'claude'):
        path = fixtures / provider; path.write_text(FIXTURE); path.chmod(0o700)
    (data / 'config.toml').write_text('shell = "/bin/bash"\nconfirm_close = false\nwarn_running_processes = false\n')
    (home / '.bashrc').write_text(f'export PATH={shlex.quote(str(fixtures))}:"$PATH"\nPS1="test $ "\n')
    endpoint = H['free_endpoint']()
    env = os.environ.copy(); env.pop('WAYLAND_DISPLAY', None)
    # A launch from a terminal of the installed Neptune must not inherit its agent channel.
    for name in [name for name in env if name.startswith(('NEPTUNE_AGENT_', 'CLAUDE'))]:
        env.pop(name)
    env.update(HOME=str(home), EGUI_INSPECTION=endpoint, NEPTUNE_AGENT_TEST_LOG=str(output / 'events.jsonl'))
    env['PATH'] = str(fixtures) + ':' + ':'.join(p for p in env['PATH'].split(':') if 'neptune-agents-' not in p)
    def git(directory, *args):
        result = subprocess.run(
            ['git', '-C', str(directory), '-c', 'user.name=Neptune Test', '-c', 'user.email=test@neptune.invalid',
             '-c', 'commit.gpgsign=false', *args], env=env, text=True, capture_output=True)
        if result.returncode:
            raise RuntimeError(f'git {args}: {result.stderr}')
        return result.stdout.strip()
    def commit(directory, name):
        (directory / name).write_text(name)
        git(directory, 'add', '--all'); git(directory, 'commit', '--quiet', '-m', name)
    git(repo, 'init', '--quiet', '--initial-branch=main')
    commit(repo, 'README.md')
    process = None
    log = (output / 'app.log').open('w')
    checks = []
    def call(*args): return H['inspect'](CLIENT, endpoint, *map(str, args))
    def wait(predicate, what, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                value = predicate()
                if value: return value
            except (OSError, ValueError, KeyError, RuntimeError, StopIteration): pass
            time.sleep(.05)
        raise AssertionError(f'Timed out waiting for {what}')
    def nodes(): return [node for _, node in call('tree')['Tree']['accesskit']['nodes']]
    def labels(): return [node['properties'].get('label', '') for node in nodes()]
    # Running text is exposed as a value, controls by their label.
    def shows(text): return lambda: any(text in str(node['properties'].get(key, '')) for node in nodes() for key in ('label', 'value'))
    def saved(): return json.loads((data / 'workspaces.json').read_text())
    def panes(): return [pane for workspace in saved()['workspaces'] for pane in workspace['panes']]
    def worktrees(): return {pane['worktree']['branch']: pane for pane in panes() if pane.get('worktree')}
    def click(label):
        node = next(node for node in nodes() if node['properties'].get('label') == label)
        bounds = node['properties']['bounds']
        call('click', (bounds['x0'] + bounds['x1']) / 2, (bounds['y0'] + bounds['y1']) / 2)
    def shot(name):
        time.sleep(.4)  # Sheets and chips fade in.
        call('screenshot', output / f'{name}.png')
    def launch(restore, size='1100x700'):
        nonlocal process
        args = [str(APP), '--data-root', str(data), '--size', size]
        if not restore: args += ['--cwd', str(repo), '--no-restore']
        process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
        H['wait_ready'](process, CLIENT, endpoint, 30)
    def close():
        click('Close window')
        process.wait(timeout=10)
    def run_command(done, what):
        # The desktop is shared: an Enter lost to the user's own input is pressed again
        # while the palette is still open.
        for _ in range(3):
            call('key', 'Enter')
            try: return wait(done, what, 5)
            except AssertionError:
                if 'Command search' not in labels(): raise
        raise AssertionError(f'Timed out waiting for {what}')
    def new_agent(branch, agent=None):
        call('key', 'g', *COMMAND)
        wait(shows('A new branch from main'), 'the sheet to find the repository')
        call('text', branch)
        if agent: click(agent)
    try:
        launch(False)
        # The palette offers it for a local terminal; the sheet asks only for a branch.
        call('key', 'p', *COMMAND)
        wait(shows('Command search'), 'the palette')
        call('text', 'worktree')
        wait(lambda: 'New agent in worktree' in ' '.join(labels()), 'the palette command')
        shot('palette')
        run_command(shows('A new branch from main'), 'the sheet to find the repository')
        shot('sheet')
        call('text', 'fix login')
        wait(shows('Creates the branch fix-login from main.'), 'the typed name to be read as a branch')
        shot('sheet-typed')
        call('key', 'Enter')
        first = wait(lambda: worktrees().get('fix-login'), 'the tab of fix-login')
        assert Path(first['cwd']) == trees / 'fix-login' and first['worktree']['repository'] == str(repo), first
        assert git(trees / 'fix-login', 'branch', '--show-current') == 'fix-login'
        wait(lambda: (worktrees()['fix-login'].get('agent') or {}).get('session_id'), 'Claude Code to open in the worktree')
        assert worktrees()['fix-login']['agent']['kind'] == 'claude'
        assert Path(worktrees()['fix-login']['agent']['cwd']) == trees / 'fix-login'
        checks.append('a branch named in the sheet becomes a tab in its own worktree with Claude Code started there')
        shot('tab')

        # A second one runs Codex; each has its own directory and branch.
        new_agent('docs/guide', 'Codex')
        shot('sheet-codex')
        call('key', 'Enter')
        wait(lambda: (worktrees().get('docs/guide', {}).get('agent') or {}).get('session_id'), 'Codex to open in its worktree')
        second = worktrees()['docs/guide']
        assert second['agent']['kind'] == 'codex' and Path(second['cwd']) == trees / 'docs-guide', second
        assert git(repo, 'branch', '--show-current') == 'main' and git(repo, 'status', '--porcelain') == ''
        checks.append('a second agent gets its own worktree and the repository checkout is untouched')
        shot('tabs')

        # Merged with nothing uncommitted: the tab offers the cleanup by itself.
        commit(trees / 'fix-login', 'login.rs')
        assert 'Remove merged worktree' not in labels()
        git(repo, 'merge', '--quiet', '--no-edit', 'fix-login')
        wait(lambda: 'Remove merged worktree' in labels(), 'the merged tab to offer cleanup', 30)
        checks.append('a merge into the base branch is noticed and offered on the tab')
        shot('merged')
        # The sheet lists the worktrees made here, with where each stands.
        call('key', 'g', *COMMAND)
        wait(lambda: 'Open worktree fix-login, merged into main' in labels() and 'Open worktree docs/guide' in labels(), 'the list of worktrees and which is merged')
        shot('sheet-list')
        call('key', 'Escape')
        wait(lambda: 'Branch name' not in labels(), 'the sheet to close')
        click('Remove merged worktree')
        wait(shows('fix-login was merged into main'), 'what removal takes')
        shot('remove-merged')
        click('Remove')
        wait(lambda: 'fix-login' not in worktrees() and not (trees / 'fix-login').exists(), 'the worktree to go')
        assert 'fix-login' not in git(repo, 'branch', '--format=%(refname:short)').splitlines()
        assert 'docs/guide' in worktrees() and (trees / 'docs-guide').exists()
        checks.append('removing a merged worktree closes its tab and deletes its folder and local branch only')

        # Work nobody committed is named, and cancelling keeps everything.
        commit(trees / 'docs-guide', 'guide.md')
        (trees / 'docs-guide' / 'draft.md').write_text('draft')
        call('key', 'p', *COMMAND)
        wait(shows('Command search'), 'the palette')
        call('text', 'remove worktree')
        wait(lambda: 'Remove worktree docs/guide' in ' '.join(labels()), 'the removal command')
        run_command(shows('1 file has changes that were never committed'), 'the uncommitted work to be named')
        assert 'Discard and remove' in labels()
        shot('remove-uncommitted')
        click('Cancel')
        wait(lambda: 'Discard and remove' not in labels(), 'the sheet to close')
        assert (trees / 'docs-guide' / 'draft.md').exists() and 'docs/guide' in worktrees()
        checks.append('uncommitted work is named before removal and cancelling keeps the worktree')

        call('resize', 640, 440)
        shot('narrow-tab')
        call('key', 'g', *COMMAND)
        wait(lambda: 'Open worktree docs/guide' in labels(), 'the narrow sheet')
        shot('narrow-sheet')
        call('key', 'Escape')
        wait(lambda: 'Branch name' not in labels(), 'the sheet to close')

        # The worktree and its agent return with the workspace.
        session = worktrees()['docs/guide']['agent']['session_id']
        close()
        launch(True)
        def resumed():
            events = [json.loads(line) for line in (output / 'events.jsonl').read_text().splitlines()]
            return any(event.get('resumed') and event.get('session') == session for event in events)
        wait(resumed, 'the agent to resume in its worktree')
        assert worktrees()['docs/guide']['agent']['session_id'] == session
        checks.append('the worktree, its branch name and its agent return when workspaces are restored')
        shot('restored')
        # Alone in view it has no tab, so the toolbar names the branch and
        # carries the offer. Removing the last terminal's worktree closes the
        # workspace with it.
        (trees / 'docs-guide' / 'draft.md').unlink()
        git(repo, 'merge', '--quiet', '--no-edit', 'docs/guide')
        other = next(pane['id'] for pane in panes() if not pane.get('worktree'))
        click(f'Terminal tab {other}')
        call('key', 'w', *COMMAND)
        wait(lambda: len(panes()) == 1, 'the other terminal to close')
        wait(lambda: 'Remove merged worktree' in labels(), 'the toolbar to offer cleanup', 30)
        shot('alone-merged')
        click('Remove merged worktree')
        wait(shows('docs/guide was merged into main'), 'what removal takes')
        click('Remove')
        wait(lambda: not trees.exists() and not saved()['workspaces'], 'the last worktree and its workspace to go')
        assert git(repo, 'branch', '--format=%(refname:short)') == 'main'
        checks.append('a worktree alone in view is offered from the toolbar, and removing it closes its workspace')
        shot('empty')
        close()
        (output / 'result.json').write_text(json.dumps({
            'status': 'passed', 'platform': sys.platform, 'features': ['inspection'],
            'provider': 'deterministic fixtures', 'checks': checks, 'address': endpoint}, indent=2))
        print(output)
    except Exception:
        if process and process.poll() is None:
            try: call('screenshot', output / 'failure.png')
            except Exception: pass
        print(output)
        raise
    finally:
        if process: H['stop_owned'](process)
        log.close()

if __name__ == '__main__': main()
