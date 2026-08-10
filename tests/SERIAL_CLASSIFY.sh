#!/bin/sh
# tests/SERIAL_CLASSIFY.sh — classify every `#[serial]`-marked test by WHY it needs
# exclusion, so the ones that need none can go back to running in parallel.
#
# Output: one `<test_fn>\t<verdict>` line per serial-marked test, sorted, where
# verdict is drawn from a closed set:
#
#   global            mutates process-wide environment (`set_var`/`remove_var`)
#   lane:cwd          changes the process working directory
#   lane:hash_kind    sets the process-wide hash kind
#   lane:<key>        already carries a named serial key; keep it
#   none              only spawns subprocesses with an explicit cwd, and uses tempdirs
#
# FAIL-CLOSED: anything this cannot decide is `global`. The cost of a wrong
# `global` is a slow test; the cost of a wrong `none` is a flaky suite.
#
# Why `none` is safe at all — three facts about this repository:
#   * `run_libra_command(args, cwd)` sets `.current_dir(cwd)` on the CHILD process
#     (`tests/command/mod.rs`), so it never touches parent state;
#   * process-wide cwd exclusion is already held by a reentrant `CWD_LOCK` inside
#     `ChangeDirGuard` (`src/utils/test.rs`), not by `#[serial]`;
#   * only a handful of test files actually call `set_var`.
set -eu
cd "$(dirname "$0")/.." || { echo "FAIL: cannot reach the repository root" >&2; exit 2; }
[ -f COMPATIBILITY.md ] && [ -d .libra ] || { echo "FAIL: not at the repository root" >&2; exit 2; }

python3 - <<'CLASSIFY_PY'
import os, re, sys

ATTR = re.compile(r'#\[(?:serial_test::)?serial(?:\(([^)]*)\))?\]')
FN   = re.compile(r'^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)')

# Helpers that pull process-wide state in on the caller's behalf. Resolved one
# level deep: the helper set is small and lives in two known files.
GLOBAL_CALLS = ('set_var', 'remove_var')
CWD_CALLS    = ('ChangeDirGuard', 'set_current_dir')
HASH_CALLS   = ('set_hash_kind',)

rows = []
for root, dirs, files in os.walk('tests'):
    dirs[:] = [d for d in dirs if d not in ('data', 'fixtures')]
    for name in sorted(files):
        if not name.endswith('.rs'):
            continue
        path = os.path.join(root, name)
        lines = open(path, encoding='utf-8', errors='replace').read().split('\n')
        for i, line in enumerate(lines):
            stripped = line.lstrip()
            if stripped.startswith('//') or stripped.startswith('*'):
                continue          # a prose mention of the attribute, not the attribute
            m = ATTR.search(line)
            if not m:
                continue
            key = (m.group(1) or '').strip()
            # the function this attribute belongs to
            j = i + 1
            while j < len(lines) and not FN.match(lines[j]):
                if not (lines[j].lstrip().startswith('#[') or lines[j].lstrip().startswith('//')
                        or not lines[j].strip()):
                    break
                j += 1
            fm = FN.match(lines[j]) if j < len(lines) else None
            if not fm:
                rows.append(('<unparsed:%s:%d>' % (path, i + 1), 'global'))
                continue
            fn = fm.group(1)
            # body: brace-matched from the signature
            depth = 0; k = j; seen = False; body = []
            while k < len(lines):
                depth += lines[k].count('{') - lines[k].count('}')
                if '{' in lines[k]:
                    seen = True
                body.append(lines[k])
                if seen and depth <= 0:
                    break
                k += 1
            text = '\n'.join(body)
            if key:
                verdict = 'lane:' + key
            elif any(c in text for c in GLOBAL_CALLS):
                verdict = 'global'
            elif any(c in text for c in HASH_CALLS):
                verdict = 'lane:hash_kind'
            elif any(c in text for c in CWD_CALLS):
                verdict = 'lane:cwd'
            elif not seen:
                verdict = 'global'          # could not delimit the body: fail closed
            else:
                verdict = 'none'
            rows.append((fn, verdict))

rows.sort()
for fn, v in rows:
    print('%s\t%s' % (fn, v))
CLASSIFY_PY
