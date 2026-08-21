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
#   lane:<key>        carries a named serial key and no process-wide pollution
#   none              only spawns subprocesses with an explicit cwd, and uses tempdirs
#
# Attributes inside a `macro_rules!` body cannot be attributed to one function;
# they are emitted as `<site:<path>:<line>>` rows judged `global` (fail-closed).
#
# Judgement order is strongest-risk-first: process-wide env/hash/cwd pollution
# forces the shared lane for that resource even when the attribute carries a
# named key — a private key cannot exclude tests sharing the polluted resource.
#
# Scanning is string/comment-aware: comments and string literals (normal, raw,
# byte/C strings, char literals) are blanked before matching, so a `#[serial]`
# inside text never produces a row, and `#[test] #[serial]` on one line is read.
#
# HEURISTIC, NOT PROOF: a `none` verdict only means the delimited function body
# does not textually contain a small blacklist of process-wide calls. Helpers
# called from the body are NOT expanded, so a `none` verdict is a deletion
# CANDIDATE only — mechanical removal waits for the strengthened classifier
# (helper expansion or unknown-call-is-global fallback), see
# docs/development/plan/plan-20260729.md DEFER-09. A wrong `global` costs a slow
# test; a wrong `none` costs a flaky suite, which is why deletion is gated.
#
# Why `none` is safe at all — three facts about this repository:
#   * `run_libra_command(args, cwd)` sets `.current_dir(cwd)` on the CHILD process
#     (`tests/command/mod.rs`), so it never touches parent state;
#   * process-wide cwd exclusion is already held by a reentrant `CWD_LOCK` inside
#     `ChangeDirGuard` (`src/utils/test.rs`), not by `#[serial]`;
#   * only a handful of test files actually call `set_var`.
set -eu
ROOT="${SERIAL_CLASSIFY_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$ROOT" || { echo "FAIL: cannot reach the repository root" >&2; exit 2; }
[ -f COMPATIBILITY.md ] && { [ -d .libra ] || [ -e .git ]; } || { echo "FAIL: not at the repository root" >&2; exit 2; }

python3 - <<'CLASSIFY_PY'
import os, re, sys

ATTR = re.compile(r'#\[(?:serial_test::)?serial(?:\(([^)]*)\))?\]')
FN   = re.compile(r'^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)')
FN_INLINE = re.compile(r'\bfn\s+([A-Za-z_][A-Za-z0-9_]*)')
RAW_STR = re.compile(r'(?:b?r)(#*)"')
CHAR_LIT = re.compile(r"'(?:\\(?:[nrt0\\'\"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\})|[^\\'])'")

# Helpers that pull process-wide state in on the caller's behalf. NOTE: only
# the delimited function body is scanned — helper bodies are NOT expanded
# (heuristic, see plan-20260729 DEFER-09).
GLOBAL_CALLS = ('set_var', 'remove_var')
CWD_CALLS    = ('ChangeDirGuard', 'set_current_dir')
HASH_CALLS   = ('set_hash_kind',)

def code_only(lines):
    """Blank comments and string literals, preserving columns and line count,
    so attribute/fn matching never sees text inside strings or comments."""
    out = []
    block_comment = 0      # nested /* */ depth
    in_string = False      # normal "..." (also b"..." / c"...") with escapes
    raw_hashes = None      # inside r#*"..."#* with this many '#'
    for line in lines:
        buf = list(line)
        i, n = 0, len(line)
        while i < n:
            if raw_hashes is not None:
                if line[i] == '"' and line.startswith('#' * raw_hashes, i + 1):
                    for k in range(1 + raw_hashes):
                        buf[i + k] = ' '
                    i += 1 + raw_hashes
                    raw_hashes = None
                    continue
                buf[i] = ' '
                i += 1
                continue
            if in_string:
                if line[i] == '\\':
                    buf[i] = ' '
                    if i + 1 < n:
                        buf[i + 1] = ' '
                    i += 2
                    continue
                buf[i] = ' '
                if line[i] == '"':
                    in_string = False
                i += 1
                continue
            if block_comment > 0:
                if line.startswith('/*', i):
                    buf[i] = buf[i + 1] = ' '
                    block_comment += 1
                    i += 2
                    continue
                if line.startswith('*/', i):
                    buf[i] = buf[i + 1] = ' '
                    block_comment -= 1
                    i += 2
                    continue
                buf[i] = ' '
                i += 1
                continue
            # code state
            if line.startswith('//', i):
                for k in range(i, n):
                    buf[k] = ' '
                break
            if line.startswith('/*', i):
                buf[i] = buf[i + 1] = ' '
                block_comment += 1
                i += 2
                continue
            cm = CHAR_LIT.match(line, i)
            if cm:
                for k in range(cm.end() - i):
                    buf[i + k] = ' '
                i = cm.end()
                continue
            rm = RAW_STR.match(line, i)
            if rm:
                for k in range(len(rm.group(0))):
                    buf[i + k] = ' '
                raw_hashes = len(rm.group(1))
                i += len(rm.group(0))
                continue
            if line[i] == '"' or line.startswith(('b"', 'c"'), i):
                if line[i] in 'bc':
                    buf[i] = ' '
                    i += 1
                buf[i] = ' '
                i += 1
                in_string = True
                continue
            i += 1
        out.append(''.join(buf))
    return out

rows = []
for root, dirs, files in os.walk('tests'):
    dirs[:] = [d for d in dirs if d not in ('data', 'fixtures')]
    for name in sorted(files):
        if not name.endswith('.rs'):
            continue
        path = os.path.join(root, name)
        lines = open(path, encoding='utf-8', errors='replace').read().split('\n')
        code = code_only(lines)
        for i, cline in enumerate(code):
            for m in ATTR.finditer(cline):
                key = (m.group(1) or '').strip()
                # the function this attribute belongs to: rest of this line,
                # else the following attribute/blank lines
                fm = FN_INLINE.search(cline, m.end())
                same_line = fm is not None
                j = i + 1
                while fm is None and j < len(code):
                    fm = FN.match(code[j])
                    if fm is None:
                        nxt = code[j].strip()
                        if nxt == '' or nxt.startswith('#['):
                            j += 1
                            continue
                        break
                if fm is None:
                    rows.append(('<site:%s:%d>' % (path, i + 1), 'global'))
                    continue
                fn = fm.group(1)
                # body: brace-matched from the signature over code-only lines
                depth = 0; seen = False; body = []
                k = i if same_line else j
                while k < len(code):
                    seg = code[k][fm.start():] if (same_line and k == i) else code[k]
                    depth += seg.count('{') - seg.count('}')
                    if '{' in seg:
                        seen = True
                    body.append(seg)
                    if seen and depth <= 0:
                        break
                    k += 1
                text = '\n'.join(body)
                if any(c in text for c in GLOBAL_CALLS):
                    verdict = 'global'
                elif any(c in text for c in HASH_CALLS):
                    verdict = 'lane:hash_kind'
                elif any(c in text for c in CWD_CALLS):
                    verdict = 'lane:cwd'
                elif key:
                    verdict = 'lane:' + key
                elif not seen:
                    verdict = 'global'          # could not delimit the body: fail closed
                else:
                    verdict = 'none'
                rows.append((fn, verdict))

rows.sort()
for fn, v in rows:
    print('%s\t%s' % (fn, v))
CLASSIFY_PY
