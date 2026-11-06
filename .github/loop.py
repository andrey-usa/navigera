"""Run a test target N times; annotate failures, write per-run rows, exit honestly.

    python loop.py recovery N    the two second-crash tests in tests/shared.rs
    python loop.py electron N    the whole Electron e2e suite

Per-run rows (sha, run, test, rc, wall s, failed tests, mode) are appended to
LOOP_ROWS_OUT (default loop-rows.json) as one JSON array for upload as an
artifact. The process exits 1 when any run failed; the green-while-failed
wrapper bug is the reason this harness exists.
"""
import json, os, re, statistics, subprocess, sys, time

kind, count = sys.argv[1], int(sys.argv[2])
linux = sys.platform.startswith('linux')
if kind == 'recovery':
    tests = ['a_second_crash_right_after_a_recovery_ends_the_session',
             'a_second_crash_the_host_reports_first_ends_the_session']
    cmds = [(t, ['cargo', 'test', '--locked', '--test', 'shared', t, '--', '--exact', '--test-threads=1', '--nocapture']) for t in tests]
else:
    cmds = [('electron', (['xvfb-run', '-a'] if linux else []) + ['cargo', 'test', '--locked', '--test', 'electron', '--', '--nocapture'])]

def mode(text):
    t = text.lower()
    if kind == 'recovery':
        if 'no navigera session' in t or 'socket is still there' in t: return 'A start/socket'
        if 'never reached the browser' in t: return 'B never reached'
        return 'other'
    if 'tab closed while this ran' in t: return 'closing window click'
    if 'notes' in t and ('tab 1 of' in t or 'orders' in t): return 'wrong window at start'
    if 'sigsegv' in t or 'signal: 11' in t or 'crashed' in t: return 'crash'
    return 'other'

def esc(s): return s.replace('%', '%25').replace('\r', '').replace('\n', '%0A')

def failed_tests(text):
    # Standard libtest per-test stdout markers.
    failed = re.findall(r'^---- (\S+) stdout ----', text, re.M)
    if failed:
        return failed
    # Fallback: the trailing summary list. A panic in a spawned thread (or a
    # binary that died before libtest printed the stdout sections) leaves no
    # marker lines, but the failed test names still end up here.
    m = re.search(r'^failures:\n((?:    \S+\n)+)', text, re.M)
    return m.group(1).split() if m else []

sha = os.environ.get('LOOP_REF', '')
rows, errors = [], 0
for run in range(1, count + 1):
    for name, cmd in cmds:
        t0 = time.perf_counter()
        try:
            p = subprocess.run(cmd, capture_output=True, text=True, timeout=1200)
            rc, text = p.returncode, p.stdout + p.stderr
        except subprocess.TimeoutExpired as e:
            rc, text = 124, f'timeout after 1200 s\n{e.stdout or ""}{e.stderr or ""}'
        wall = time.perf_counter() - t0
        failed = failed_tests(text)
        panics = []
        for m in re.finditer(r'panicked at ([^\n]*)\n((?:[^\n]*\n){0,3})', text):
            panics.append((m.group(1) + ' ' + m.group(2).strip()).strip()[:500])
        m = mode(text) if rc else 'pass'
        rows.append(dict(sha=sha, name=name, run=run, rc=rc, wall=round(wall, 2), failed=failed, mode=m))
        print(json.dumps(rows[-1]), flush=True)
        if rc:
            print(text[-6000:], flush=True)
            if errors < 9:
                errors += 1
                print(f'::error title=run {run} {name} rc {rc} ({m})::{esc(" | ".join(failed) + chr(10) + chr(10).join(panics[:3]) or text[-1200:])}', flush=True)

summary = {}
for name, _ in cmds:
    r = [x for x in rows if x['name'] == name]
    per_test = {}
    for x in r:
        for f in x['failed']: per_test[f] = per_test.get(f, 0) + 1
    modes = {}
    for x in r:
        if x['rc']: modes[x['mode']] = modes.get(x['mode'], 0) + 1
    summary[name] = dict(failed_runs=f"{sum(1 for x in r if x['rc'])}/{len(r)}", per_test=per_test, modes=modes,
                         wall_median=round(statistics.median(x['wall'] for x in r), 2), wall_max=max(x['wall'] for x in r))
print(f'::notice title=summary {kind} {os.environ.get("RUNNER_OS", "")} {sha}::{json.dumps(summary)}', flush=True)
print(json.dumps(summary, indent=1))

with open(os.environ.get('LOOP_ROWS_OUT', 'loop-rows.json'), 'w', encoding='utf-8') as f:
    json.dump(rows, f, indent=1)

sys.exit(1 if any(x['rc'] for x in rows) else 0)
