"""Local duplicate-normative checker for plan-20260729.md.
Finds the failure mode that has produced ~60% of Codex findings:
the same contract stated in more than one place."""
import io, re, sys
s = io.open("docs/development/plan/plan-20260729.md", encoding="utf-8").read()
# exclude the revision-history and review-log tables (historical records are allowed to repeat)
hist_start = s.index("### 修订历史")
hist_end = s.index("## 已决议设计决策")
log_start = s.index("## Codex review log")
body = s[:hist_start] + s[hist_end:log_start] + s[s.index("## 非目标与延后项"):]

problems = []
def one_of(pattern, label, allowed=1):
    hits = re.findall(pattern, body)
    if len(hits) > allowed:
        problems.append(f"{label}: {len(hits)} normative statements (allowed {allowed})")

# 1) per-card path-count phrasing (三条/四条/五条/八条具名路径)
for n in ["三条", "四条", "五条", "六条", "八条"]:
    hits = re.findall(rf"{n}(?:具名)?路径", body)
    if hits:
        problems.append(f"path-count phrase '{n}路径' appears {len(hits)}x — counts must come from the write-set field")
# 2) legacy schema field
one_of(r"libra_surface_status", "legacy field libra_surface_status", 0)
# 3) CTF dependency arrows outside the canonical table
arrows = re.findall(r"CT3-04 -> CTF-P01|CTF-P0n -> CT3-02|CT3-02 -> CTF-C01|CTF-C0n -> CT4-01", body)
if len(arrows) > 4:
    problems.append(f"CTF DAG arrows stated {len(arrows)}x outside the single table")
# 4) member-count prose
for n in ["七张", "八张", "九张", "十张", "十一张", "十二张"]:
    hits = re.findall(rf"{n}(?:固定)?(?:成员)?卡", body)
    if hits:
        problems.append(f"member-count phrase '{n}卡' appears {len(hits)}x — must reference the REL-01 ID set")
# 5) fail-open shell patterns
for pat, label in [(r"\|\| true", "|| true"), (r"\| LC_ALL=C sort", "piped sort"), (r"\|\| echo 0", "|| echo 0")]:
    hits = re.findall(pat, body)
    if hits:
        problems.append(f"fail-open pattern {label} appears {len(hits)}x")
# 6) no-release used normatively
# only AFFIRMATIVE assignments count; "本计划不使用 `no-release`" is the desired statement
aff = [l for l in body.split("\n") if "`no-release`" in l and not re.search(r"不使用|不适用|禁止|改为|由 `no-release`", l)]
if aff:
    problems.append(f"`no-release` used affirmatively in {len(aff)} line(s)")

print("\n".join(problems) if problems else "CLEAN: no duplicate-normative or fail-open patterns")
sys.exit(1 if problems else 0)
