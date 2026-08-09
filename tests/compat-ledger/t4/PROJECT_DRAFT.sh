#!/bin/sh
# plan-20260729 CT3-06 — project the frozen draft onto the active scenario set,
# and the ONLY implementation of that projection. CT3-04's precheck and
# CT3-02's verbatim-draft gate both call this.
#
# Why a projector exists at all: the draft is frozen before the precheck runs,
# but the precheck may reclassify a scenario out of `direct`. The active set is
# therefore a DERIVED quantity — the frozen snapshot intersected with the
# ledger's current `direct` set — and it is never written to the repository.
# Projecting means emitting the frozen draft with the functions of dropped
# scenarios removed, so what runs is always a subset of what was frozen and
# never a rewrite of it.
#
# Two properties the gates depend on:
#   * determinism — the same (draft, active set) yields byte-identical output,
#     so a projection can be diffed against a previous one;
#   * empty-drop identity — projecting with nothing dropped reproduces the
#     draft byte for byte, so the projector cannot quietly alter the frozen
#     text on the happy path.
#
# Usage:
#   DRAFT=<draft file> ACTIVE=<file of active test_fn names, one per line> \
#     sh PROJECT_DRAFT.sh > projected.rs
#
# The four fixed guards are never dropped: they are the integrity checks over
# the ported suite itself, not migrated scenarios, so they are not in the
# snapshot and must survive every projection.
set -eu

: "${DRAFT:?set DRAFT to the frozen draft file}"
: "${ACTIVE:?set ACTIVE to the file of active test function names}"
[ -f "$DRAFT" ] || { echo "FAIL: no such DRAFT: $DRAFT" >&2; exit 1; }
[ -f "$ACTIVE" ] || { echo "FAIL: no such ACTIVE: $ACTIVE" >&2; exit 1; }

GUARDS="t4_port_integrity t4_port_no_foreign_harness t4_port_document_scope_and_boundaries t4_port_direct_rows_have_tests"

python3 - "$DRAFT" "$ACTIVE" "$GUARDS" <<'PROJECT_DRAFT_PY'
import re
import sys

draft_path, active_path, guards = sys.argv[1], sys.argv[2], sys.argv[3].split()
text = open(draft_path, encoding="utf-8").read()
active = {l.strip() for l in open(active_path, encoding="utf-8") if l.strip()}
keep_always = set(guards)

lines = text.split("\n")
FN = re.compile(r"^\s*(?:pub\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")

# A test item is its attribute run (#[test], #[serial], doc comments) plus the
# function that follows, up to the brace that closes it. Splitting on blank
# lines would be wrong: a scenario body may contain them.
out = []
i = 0
n = len(lines)
while i < n:
    # Collect a candidate attribute/doc-comment run.
    start = i
    while i < n and (lines[i].lstrip().startswith("#[") or lines[i].lstrip().startswith("///")):
        i += 1
    m = FN.match(lines[i]) if i < n else None
    if m is None:
        # Not an item head: emit the run and the line, and move on.
        for j in range(start, min(i + 1, n)):
            out.append(lines[j])
        i = min(i + 1, n)
        continue

    name = m.group(1)
    # Walk to the end of the function body by brace depth, ignoring braces that
    # appear inside string literals or line comments.
    depth = 0
    j = i
    seen_open = False
    while j < n:
        line = lines[j]
        in_str = False
        k = 0
        while k < len(line):
            c = line[k]
            if in_str:
                if c == "\\":
                    k += 2
                    continue
                if c == '"':
                    in_str = False
            else:
                if c == '"':
                    in_str = True
                elif c == "/" and k + 1 < len(line) and line[k + 1] == "/":
                    break
                elif c == "{":
                    depth += 1
                    seen_open = True
                elif c == "}":
                    depth -= 1
            k += 1
        j += 1
        if seen_open and depth == 0:
            break

    if name in keep_always or name in active:
        out.extend(lines[start:j])
    # else: the scenario was reclassified out of `direct`; drop the whole item.
    i = j

sys.stdout.write("\n".join(out))
PROJECT_DRAFT_PY
