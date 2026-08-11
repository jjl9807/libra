#!/bin/sh
# tests/compat-ledger/t4/CT302_GATE.sh —— CT3-02 净室门唯一入口（本卡交付）。
# 内容 = 卡内六个 fenced block 依序内联：⓪ 冻结输入断言 / ① 承诺清单门 /
# ② 净室相似度 / 负例门 / 篡改负例段 / ⑤ 变更集 token 筛子，前后夹权威头尾。
# 组装契约：被内联的 block 一律删除自身的 `mktemp -d` 与 trap 行——shell 只保留最后
# 一次设置的 trap，逐块各自设置会让顶层 RUN_DIR 在成功/失败/信号三条路径上全部泄漏。

set -eu
export LIBRA_SKIP_WEB_BUILD=1
cd "$(dirname "$0")/../../.." || { echo "FAIL: cannot cd to the repository root"; exit 2; }
[ -f COMPATIBILITY.md ] && [ -d .libra ] \
|| { echo "FAIL: not at the Libra repository root"; exit 2; }   # R73 P1：仓库根校验写进头部
unset TARGET PHRASE_ALLOWLIST PHRASE_SIDECAR                       # R73 P1：拒绝调用方环境覆盖
RUN_DIR=$(mktemp -d); export RUN_DIR                               # R73 P1：内联 block 依赖它
GD="$RUN_DIR/gd"; RD="$RUN_DIR/rd"; SD="$RUN_DIR/sd"; TDIR="$RUN_DIR/tdir"
mkdir -p "$GD" "$RD" "$SD" "$TDIR"        # ← 全部目录变量在此**显式初始化**（`set -u` 安全）
# GC-15 的两个 helper（`snapshot_frozen` / `verify_frozen`）在此**逐字内联**——见「全局工程约束」
# 的 GC-15 实现块；两个交付脚本各自持有一份完全相同的定义，调用点在下方各 block 内。
ct302_done=0        # 见 `ct302_cleanup`：没有它，`${VAR:?}` 中止会被报成成功
ct302_cleanup() {
r=$?; [ -n "${1:-}" ] && r=$1
trap - EXIT INT TERM
set +e
# **本卡订正（偏离权威头部，已在任务记录登记）**：`${VAR:?msg}` 触发的中止会让 shell 退出，
# 但 EXIT trap 入口处的 `$?` 仍是 **0**——handler 于是原样 `exit 0`，一次硬失败被报成通过。
# 已用五行脚本复现。改为 fail-closed：脚本走到末尾才置 `ct302_done=1`，否则 `r=0` 一律抬到 2。
if [ "$r" -eq 0 ] && [ "${ct302_done:-0}" -ne 1 ]; then
  echo "FATAL: the gate exited before reaching its end (a \${VAR:?} abort or similar) — refusing to report success" >&2
  r=2
fi
# R75 P1：删除失败不得被成功退出码掩盖——门通过但快照与中间证据留在盘上却报 0，
# 下一次运行会踩到残留目录，而任务记录里没有任何痕迹。失败即抬到 3（fail-closed）。
if ! rm -rf "$RUN_DIR"; then          # R77 P1（自审）：与 `_gd_cleanup` 同型，改为 if 结构
  echo "FATAL: cannot remove $RUN_DIR — evidence left on disk" >&2
  if [ "$r" -eq 0 ]; then r=3; fi
fi
exit "$r"
}
trap 'ct302_cleanup' EXIT; trap 'ct302_cleanup 130' INT; trap 'ct302_cleanup 143' TERM

# ---- GC-15 冻结锚点「按提交快照消费」协议的唯一实现（逐字内联）----
# snapshot_frozen <run-dir> <path>...  —— 断言已跟踪且无未提交改动，再把 HEAD blob 快照到私有目录
# **R76 P0**：本函数总是作为 `||` 的左操作数被调用，`set -e` 在其内部完全失效——所以**每一条**
# 命令都必须自带 `|| return 1`。原文的 `>> "$_rd/snap.manifest"` 没有：磁盘写满/权限变化时，
# 已写入的若干行仍在，函数继续跑到 `unset` 并返回 0；`verify_frozen` 又只拿被截断的清单自比，
# 于是「已验条数 == 清单行数」成立而漏掉的锚点**从未被检查**。改为逐条 guard + 请求数校验。
snapshot_frozen() {
  _rd=$1; shift
  _want_n=$#
  mkdir -p "$_rd/snap" || { echo "FAIL: cannot create the snapshot dir" >&2; return 1; }
  : > "$_rd/snap.manifest" || return 1
  for _f in "$@"; do
    libra ls-files --cached --error-unmatch -- "$_f" > /dev/null \
      || { echo "FAIL: frozen input is not tracked: $_f" >&2; return 1; }
    _st=$(libra status --porcelain=v1 -- "$_f") || { echo "FAIL: libra status failed on $_f" >&2; return 1; }
    [ -z "$_st" ] || { echo "FAIL: frozen input has uncommitted modifications: $_f" >&2; return 1; }
    mkdir -p "$_rd/snap/$(dirname "$_f")" || return 1
    libra show "HEAD:$_f" > "$_rd/snap/$_f" \
      || { echo "FAIL: cannot snapshot HEAD:$_f" >&2; return 1; }
    _h=$(shasum -a 256 "$_rd/snap/$_f") || { echo "FAIL: cannot hash the snapshot of $_f" >&2; return 1; }
    printf '%s  %s\n' "${_h%% *}" "$_f" >> "$_rd/snap.manifest" \
      || { echo "FAIL: cannot append $_f to the snapshot manifest" >&2; return 1; }
  done
  # R76 P0：清单行数必须等于**请求快照的路径条数**——只比「写了几行」无法发现整行丢失
  _got_n=$(wc -l < "$_rd/snap.manifest") || { echo "FAIL: cannot count the snapshot manifest" >&2; return 1; }
  [ "$_got_n" -eq "$_want_n" ] \
    || { echo "FAIL: snapshotted $_got_n of $_want_n frozen inputs" >&2; return 1; }
  unset _rd _f _st _h _want_n _got_n
}
# verify_frozen <run-dir> —— 消费窗口结束时复验：状态仍为空且工作树内容仍等于快照
# **R75 P0：本函数此前在三种情形下静默「通过」**——① 清单为空（零次迭代直接走到 `unset`）；
# ② 清单文件不存在（重定向失败，但本函数总是作为 `||` 的左操作数被调用，`set -e` 在其内部
# 完全失效，于是继续执行到 `unset` 并返回 0）；③ 某行解析不出 `_f` 而被 `continue` 跳过。
# 三种都会让调用点打印「every frozen anchor is unchanged」而实际一个都没验。改为 fail-closed：
# 显式确认清单可读且非空、逐行强制两字段、末尾断言**已验条数 == 清单行数**。
verify_frozen() {
  _rd=$1
  [ -r "$_rd/snap.manifest" ] \
    || { echo "FAIL: snapshot manifest is missing or unreadable: $_rd/snap.manifest" >&2; return 1; }
  _total=$(wc -l < "$_rd/snap.manifest") || { echo "FAIL: cannot count the snapshot manifest" >&2; return 1; }
  [ "$_total" -gt 0 ] || { echo "FAIL: snapshot manifest is empty — nothing was snapshotted" >&2; return 1; }
  _seen=0
  while IFS=' ' read -r _want _f; do
    [ -n "$_want" ] && [ -n "$_f" ] \
      || { echo "FAIL: malformed snapshot manifest record" >&2; return 1; }
    _st=$(libra status --porcelain=v1 -- "$_f") \
      || { echo "FAIL: libra status failed on $_f" >&2; return 1; }
    [ -z "$_st" ] || { echo "FAIL: $_f changed during the consumption window" >&2; return 1; }
    _now=$(shasum -a 256 "$_f") || { echo "FAIL: cannot hash $_f" >&2; return 1; }
    [ "${_now%% *}" = "$_want" ] \
      || { echo "FAIL: $_f content changed during the consumption window" >&2; return 1; }
    _seen=$((_seen+1))
  done < "$_rd/snap.manifest"
  [ "$_seen" -eq "$_total" ] \
    || { echo "FAIL: verified $_seen of $_total frozen anchors" >&2; return 1; }
  unset _rd _want _f _st _now _total _seen
}

# ============ ⓪ 冻结输入的 committed-and-unmodified 断言 ============
: "${RUN_DIR:?export a caller-owned RUN_DIR}"
# R70 P1：本卡还消费 `PROBES.allow` 与 `REPLAY_SOURCES.sh`（GC-14 的白名单与唯一重放实现），
# 二者同样必须已提交且无未提交改动——否则临时改白名单授权伪造探针、或改重放器再跑门，
# 证据能过而提交里没有这些改动，提交后不可复现。
for f in tests/compat-ledger/PHRASE_ALLOWLIST.txt tests/compat-ledger/PHRASE_ALLOWLIST.sha256 \
         tests/compat-ledger/t4/CLEANROOM.sh tests/compat-ledger/t4/DIRECT_SNAPSHOT.tsv \
         tests/compat-ledger/t4/DRAFT.rs.txt tests/compat-ledger/t4/PROJECT_DRAFT.sh \
         tests/compat-ledger/t4/PROBES.allow tests/compat-ledger/t4/REPLAY_SOURCES.sh; do
  [ -s "$f" ] || { echo "FAIL: frozen input missing or empty: $f"; exit 1; }
  # R58 P1：空的 `libra status` **不能**证明已提交——被 ignore 或配置隐藏的 untracked 文件
  # 同样产出空状态（`docs/commands/status.md:51-52,354-372`）。先证明它在 index 里。
  libra ls-files --cached --error-unmatch -- "$f" > /dev/null \
    || { echo "FAIL: frozen input is not tracked (never committed): $f"; exit 1; }
  st=$(libra status --porcelain=v1 -- "$f") || { echo "FAIL: libra status failed on $f"; exit 1; }
  [ -z "$st" ] || { echo "FAIL: frozen input has uncommitted modifications: $f"; exit 1; }
done
echo "OK: all frozen inputs are tracked, committed and unmodified"
# **R73 P1：`snapshot_frozen`/`verify_frozen` 的定义必须逐字写在本脚本头部**（GC-15 的实现块），
# 否则首次调用即 `command not found`；本卡的权威组装头部已包含这两段函数定义。
# **GC-15**：随即把这些 blob 快照到私有目录，本卡后续**只消费快照副本**（消除瞬时检查之后的
# 篡改窗口）；`EXPECTED.txt` 等实际被消费的冻结输入一并纳入。窗口结束时 `verify_frozen`。
snapshot_frozen "$RUN_DIR" \
  tests/compat-ledger/PHRASE_ALLOWLIST.txt tests/compat-ledger/PHRASE_ALLOWLIST.sha256 \
  tests/compat-ledger/t4/CLEANROOM.sh tests/compat-ledger/t4/DIRECT_SNAPSHOT.tsv \
  tests/compat-ledger/t4/DRAFT.rs.txt tests/compat-ledger/t4/PROJECT_DRAFT.sh \
  tests/compat-ledger/t4/PROBES.allow tests/compat-ledger/t4/REPLAY_SOURCES.sh \
  tests/compat-ledger/t4/EXPECTED.txt tests/compat-ledger/t4/MANIFEST.tsv \
  || { echo "FAIL: cannot snapshot the frozen inputs (GC-15)"; exit 1; }
# **R73 P1（GC-15 ③ 落地）**：快照之后本卡**一律经下列变量寻址**，不得再出现工作树字面路径
# ——否则 ② 的快照只是摆设，门仍然消费可被并发改写的工作树副本。
SNAPD="$RUN_DIR/snap"
A_CLEANROOM="$SNAPD/tests/compat-ledger/t4/CLEANROOM.sh"
A_EXPECTED="$SNAPD/tests/compat-ledger/t4/EXPECTED.txt"
# 组装接线（本卡）：卡内 block ② 与负例段都要求调用方导出 `EXPECTED_SNAP`，而 ⓪ 段产出的
# 是 `A_EXPECTED`。两者必须在此显式接上，否则 block ② 的 `${EXPECTED_SNAP:?}` 会中止脚本。
EXPECTED_SNAP="$A_EXPECTED"; export EXPECTED_SNAP
A_PROJECT="$SNAPD/tests/compat-ledger/t4/PROJECT_DRAFT.sh"
A_REPLAY="$SNAPD/tests/compat-ledger/t4/REPLAY_SOURCES.sh"
A_PROBES="$SNAPD/tests/compat-ledger/t4/PROBES.allow"
A_DRAFT="$SNAPD/tests/compat-ledger/t4/DRAFT.rs.txt"
A_DIRECT="$SNAPD/tests/compat-ledger/t4/DIRECT_SNAPSHOT.tsv"
A_PA="$SNAPD/tests/compat-ledger/PHRASE_ALLOWLIST.txt"
A_SC="$SNAPD/tests/compat-ledger/PHRASE_ALLOWLIST.sha256"
export A_PA A_SC
# `CLEANROOM.sh` 的 `PHRASE_ALLOWLIST`/`PHRASE_SIDECAR` **默认值指向工作树**，故每次调用都必须
# 显式传快照路径（篡改负例段除外——它按设计传自己的临时副本）。

# ============ ① `t4_port_*` 承诺清单与执行 ============
# `$GD` 的清理由权威头部的 `ct302_cleanup` 统一承担（组装契约：内联 block 不得自设 trap）。
. ./.env.test                     # R37 P1：首个 cargo 调用（--list）前即加载测试环境
cargo test --test command_test t4_port -- --list > "$GD/list_t4port.txt" \
  || { echo "FAIL: cargo test --list failed"; exit 1; }
# R35 P1：不得用 grep/sed 假解析 TOML（合法的多行数组会被漏掉）——承诺清单改由 CT2-01 交付的
# TOML 解析器守卫 `ledger_dump_libra_tests` 机器输出派生（逐行 `LIBRA_TEST <fn>`），拆管道取数。
# R40 P1：两个解析器过滤先各自 --list 锁定具名函数（零匹配的 0==0 假绿封堵）
cargo test --test compat_ledger_schema ledger_dump -- --list > "$GD/list_dump.txt" \
  || { echo "FAIL: --list failed"; exit 1; }
# R64 P1：锚定到模块边界并要求**恰一次**命中（裸子串可被 `fake_ledger_dump_libra_tests` 冒充）
if command grep -cE '(^|::)ledger_dump_libra_tests: test$' "$GD/list_dump.txt" > "$GD/nhit.txt"; then :; else
  rc=$?; [ "$rc" -eq 1 ] || { echo "FAIL: grep failed with $rc"; exit "$rc"; }
  echo 0 > "$GD/nhit.txt"
fi
[ "$(cat "$GD/nhit.txt")" -eq 1 ] || { echo "FAIL: ledger_dump_libra_tests guard missing"; exit 1; }
# R64 P1：锚定到模块边界并要求**恰一次**命中（裸子串可被 `fake_ledger_dump_direct_ids` 冒充）
if command grep -cE '(^|::)ledger_dump_direct_ids: test$' "$GD/list_dump.txt" > "$GD/nhit.txt"; then :; else
  rc=$?; [ "$rc" -eq 1 ] || { echo "FAIL: grep failed with $rc"; exit "$rc"; }
  echo 0 > "$GD/nhit.txt"
fi
[ "$(cat "$GD/nhit.txt")" -eq 1 ] || { echo "FAIL: ledger_dump_direct_ids guard missing"; exit 1; }
cargo test --test compat_ledger_schema ledger_dump_libra_tests -- --exact --nocapture > "$GD/lt.out" \
  || { echo "FAIL: cannot dump libra_tests via ledger parser"; exit 1; }
sed -n 's/^LIBRA_TEST //p' "$GD/lt.out" > "$GD/names_t4port.raw" || { echo "FAIL: parse dump"; exit 1; }
[ -s "$GD/names_t4port.raw" ] || { echo "FAIL: parser produced zero LIBRA_TEST rows"; exit 1; }
LC_ALL=C sort -u "$GD/names_t4port.raw" > "$GD/names_t4port.txt"
echo t4_port_direct_rows_have_tests >> "$GD/names_t4port.txt"
echo t4_port_tests_document_scope_and_boundaries >> "$GD/names_t4port.txt"
echo t4_port_integrity >> "$GD/names_t4port.txt"
echo t4_port_no_foreign_harness >> "$GD/names_t4port.txt"
LC_ALL=C sort -u -o "$GD/names_t4port.txt" "$GD/names_t4port.txt"
want=$(command grep -c . "$GD/names_t4port.txt")
# 承诺清单必须真的来自账本：四个守卫名是硬加的（唯一清单 = PRECHECK.sh 的 GUARDS，
# 两处必须一致），所以「>= 1」恒成立、不构成判据。
# 这里断言**从账本推导出的迁移测试**至少与账本 direct 行数相等。
# R35：direct 行数同样由 TOML 解析器守卫派生（不 grep 文本），拆管道取数
cargo test --test compat_ledger_schema ledger_dump_direct_ids -- --exact --nocapture > "$GD/direct.out" \
  || { echo "FAIL: cannot dump direct ids via ledger parser"; exit 1; }
# R61 P1：`sed | cut` 的退出码只取 cut——sed 中途失败但已输出部分行仍会通过。拆两步各查。
sed -n 's/^DIRECT_ID //p' "$GD/direct.out" > "$GD/direct.rows" || { echo "FAIL: sed DIRECT_ID"; exit 1; }
cut -f1 "$GD/direct.rows" > "$GD/direct.raw" || { echo "FAIL: cut DIRECT_ID col1"; exit 1; }
[ -s "$GD/direct.raw" ] || { echo "FAIL: parser produced zero DIRECT_ID rows"; exit 1; }
direct=$(wc -l < "$GD/direct.raw")
# 守卫数量从 PRECHECK.sh 的 GUARDS 唯一清单**机械派生**（R31 P0：不得手写数字）。
# R55 自审 P2 澄清：只有 `gn` 是派生量；等式另一侧的 `want` 来自上方**手写的具名清单**
# （含四个守卫名）。新增第五个守卫时必须**同批**把它加进该清单，否则 `want - gn` 会比
# `direct` 少 1 并在此 fail-closed 报错——本门不会假绿，但修法是补清单而非改本行。
# R54 P1：`sed … &#124; grep -c` 在 POSIX `set -e` 下只取 grep 的退出码——sed 读取中途失败但
# 已输出一行时仍会得到 gn≥1 并通过。改为「sed 独立落盘 + 显式检查退出码」，再对文件计数。
sed -n '/GUARDS="/,/"$/p' tests/compat-ledger/t4/PRECHECK.sh > "$GD/guards_block.txt" \
  || { echo "FAIL: cannot extract GUARDS block from PRECHECK.sh"; exit 1; }
[ -s "$GD/guards_block.txt" ] || { echo "FAIL: GUARDS block is empty in PRECHECK.sh"; exit 1; }
if command grep -c 't4_port_' "$GD/guards_block.txt" > "$GD/gn.txt"; then :; else
  rc=$?; [ "$rc" -eq 1 ] || { echo "FAIL: grep (guard count) failed with $rc"; exit "$rc"; }
  echo 0 > "$GD/gn.txt"
fi
gn=$(cat "$GD/gn.txt")
[ "$gn" -ge 1 ] || { echo "FAIL: derived guard count is zero"; exit 1; }
[ "$((want - gn))" -eq "$direct" ] \
  || { echo "FAIL: ledger has $direct direct rows but $((want - gn)) migrated tests promised (must be equal)"; exit 1; }
if command grep -c ': test$' "$GD/list_t4port.txt" > "$GD/nt.txt"; then :; else
  rc=$?; [ "$rc" -eq 1 ] || { echo "FAIL: grep --list failed with $rc"; exit "$rc"; }; echo 0 > "$GD/nt.txt"
fi
n=$(cat "$GD/nt.txt")
[ "$n" -ge "$want" ] || { echo "FAIL: --list found $n tests, expected >= $want"; exit 1; }
while read -r fn; do
  # R63 P1：锚定到模块边界并要求**恰一次**命中（裸子串匹配可被同后缀假测试冒充）
  if command grep -cE "(^|::)${fn}: test$" "$GD/list_t4port.txt" > "$GD/nhit.txt"; then :; else
    rc=$?; [ "$rc" -eq 1 ] || { echo "FAIL: grep failed with $rc"; exit "$rc"; }
    echo 0 > "$GD/nhit.txt"
  fi
  [ "$(cat "$GD/nhit.txt")" -eq 1 ] \
    || { echo "FAIL: promised test $fn does not resolve to exactly one --list entry"; exit 1; }
done < "$GD/names_t4port.txt"
# R40 P1：整个承诺集合（含 integrity 自身）零 #[ignore]——ignored 的 integrity 会静默失去检查力
cargo test --test command_test t4_port -- --ignored --list > "$GD/ign_t4port.txt" \
  || { echo "FAIL: --ignored --list failed"; exit 1; }
if command grep -n ': test$' "$GD/ign_t4port.txt"; then
  echo "FAIL: t4_port tests are #[ignore]d"; exit 1
else
  rc=$?; [ "$rc" -eq 1 ] || { echo "FAIL: grep failed with $rc"; exit "$rc"; }
fi
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test t4_port

# ============ ② 净室相似度（长子串 + 8-token n-gram + token 筛子） ============
# **ER-11（自审 P1）**：本脚本是 CT3-06 的**已提交交付物**，随推送公开——绝不能内联任何
# 审阅者的私有绝对路径。与 `SELECTION.sh` 同口径，改由调用方 export，缺失即失败。
: "${GRIT_REPO:?set GRIT_REPO to the grit checkout (never hard-code an absolute path)}"
_tmp_c() {
  r=$?; [ -n "${1:-}" ] && r=$1
  set +e
  if ! rm -rf "$GD"; then
    echo "FATAL: cannot remove $GD — intermediate files left on disk" >&2
    if [ "$r" -eq 0 ]; then r=3; fi
  fi
  exit "$r"
}
PIN=dfb079967b9cbc99e533c21e65f674bb3f5e8b07
[ "$(git -C "$GRIT_REPO" rev-parse HEAD)" = "$PIN" ] || { echo "FAIL: grit HEAD != pin"; exit 1; }
st=$(git -C "$GRIT_REPO" status --porcelain -- tests) \
  || { echo "FAIL: git status failed in $GRIT_REPO"; exit 1; }
[ -z "$st" ] || { echo "FAIL: grit tests/ is dirty; overlap check needs a clean pinned tree"; exit 1; }
# **语料按内容绑定（自审 P2）**：`rev-parse HEAD == PIN` + `status --porcelain` 为空**不足以**
# 证明工作区文件就是 pin 里的内容——`skip-worktree` / `assume-unchanged` 的条目既不出现在
# status，也不影响 HEAD。故：① 拒绝任何 `S`/`h` 标记；② 对 `EXPECTED.txt` 的 12 个源逐个
# 比对工作区文件与 pin 中 blob 的 sha256。
git -C "$GRIT_REPO" ls-files -v -- tests > "$GD/grit_lsfiles.txt" \
  || { echo "FAIL: git ls-files -v failed in the grit checkout"; exit 1; }
if command grep -nE '^[Sh]' "$GD/grit_lsfiles.txt"; then
  echo "FAIL: grit tests/ has skip-worktree / assume-unchanged entries — corpus is not trustworthy"; exit 1
else
  rc=$?; [ "$rc" -eq 1 ] || { echo "FAIL: grep failed with $rc"; exit "$rc"; }
fi
# R79 P1：stem 清单只读冻结的 EXPECTED_SNAP（调用方 GC-15 快照），禁止工作树字面路径
: "${EXPECTED_SNAP:?export EXPECTED_SNAP to a frozen copy of tests/compat-ledger/t4/EXPECTED.txt (GC-15)}"
[ -s "$EXPECTED_SNAP" ] || { echo "FAIL: EXPECTED_SNAP missing or empty: $EXPECTED_SNAP"; exit 1; }
export EXPECTED_SNAP
while read -r stem; do
  [ -n "$stem" ] || continue
  f="tests/$stem"
  wt=$(shasum -a 256 "$GRIT_REPO/$f") || { echo "FAIL: cannot hash $f in the worktree"; exit 1; }
  git -C "$GRIT_REPO" show "$PIN:$f" > "$GD/pin_blob" \
    || { echo "FAIL: $f is not present at the pin"; exit 1; }
  pb=$(shasum -a 256 "$GD/pin_blob") || { echo "FAIL: cannot hash the pinned blob of $f"; exit 1; }
  [ "${wt%% *}" = "${pb%% *}" ] \
    || { echo "FAIL: $f differs from the pinned blob — corpus substituted"; exit 1; }
done < "$EXPECTED_SNAP"
# **R72 P1（TOCTOU）**：摘要核对之后**不得再打开工作树文件**——源文件可在检查后被临时替换、
# 扫描结束前还原。逐个把 pin blob 物化到私有快照目录，后续重叠扫描**只读这些副本**。
mkdir -p "$GD/grit" || { echo "FAIL: cannot create the grit snapshot dir"; exit 1; }
while read -r stem; do
  [ -n "$stem" ] || continue
  mkdir -p "$GD/grit/$(dirname "tests/$stem")" || { echo "FAIL: mkdir snapshot"; exit 1; }
  git -C "$GRIT_REPO" show "$PIN:tests/$stem" > "$GD/grit/tests/$stem" \
    || { echo "FAIL: cannot materialise the pinned blob for $stem"; exit 1; }
done < "$EXPECTED_SNAP"
GRIT_SNAPSHOT="$GD/grit"; export GRIT_SNAPSHOT   # 下方扫描器一律读 $GRIT_SNAPSHOT，不读 $GRIT_REPO
# **R78/R79 P1**：Python 只读 GRIT_SNAPSHOT；EXPECTED_SNAP 已在物化循环前强制要求
GRIT_SNAPSHOT="$GRIT_SNAPSHOT" EXPECTED_SNAP="$EXPECTED_SNAP" \
  TARGET="${TARGET:-tests/command/t4_port_test.rs}" python3 - <<'PY'
import difflib, os, re, sys
grit = os.environ["GRIT_SNAPSHOT"]   # R78：只读 pin 物化副本

def norm(t):
    # 2026-07-29（ADR-CT-06 同批）：**不剥离注释**。Rust 的 `///` 以 `//` 开头，
    # 在「测试必须写详细注释」的契约下，抄上游描述是最省事的写法，剥离注释会让这条通道免检。
    return re.sub(r"\s+", " ", t).lower()

def grams(t, n=8):
    w = t.split()
    return {" ".join(w[i:i+n]) for i in range(max(0, len(w) - n + 1))}

bad = 0
# R29 P0：检查目标由 TARGET 环境变量指定（sh 包装层已给缺省值）——CT3-02 查最终测试
# 文件、CT3-04 查本轮活跃投影 `$RUN_DIR/draft.active.rs`、负例门查负例样本，三者共用本实现
targets = [os.environ["TARGET"]]
# token 筛子（三层之一，与长子串/n-gram 同一退出点；上游 harness 标识符的唯一清单）
TOKENS = ["test_expect_success", "test_expect_failure", "test_cmp",
          "test_when_finished", "TEST_DIRECTORY", "test-lib.sh"]
exp = os.environ.get("EXPECTED_SNAP", "tests/compat-ledger/t4/EXPECTED.txt")
if not os.path.exists(exp):
    print("FAIL: EXPECTED.txt missing"); sys.exit(1)
# grit 已是 $GRIT_SNAPSHOT 根，其下 layout 为 tests/<stem>
sources = [os.path.join(grit, "tests", l.strip()) for l in open(exp) if l.strip()]
if not sources:
    print("FAIL: EXPECTED.txt is empty"); sys.exit(1)

# 白名单与其摘要 sidecar 无条件必须存在（CT2-03 交付物，R35 拆分；R31 P1——不存在「无白名单」的
# 合法状态，缺任一/格式坏/不匹配一律失败，不依赖执行期环境变量）
import hashlib
# R39：路径可经环境变量指向**临时副本**（篡改负例门专用）；缺省即为 CT2-03 交付的原文件
ap = os.environ.get("PHRASE_ALLOWLIST", "tests/compat-ledger/PHRASE_ALLOWLIST.txt")   # CT2-03 交付，CT3-02 不可改
sc = os.environ.get("PHRASE_SIDECAR", "tests/compat-ledger/PHRASE_ALLOWLIST.sha256")
if not os.path.exists(ap):
    print("FAIL: PHRASE_ALLOWLIST.txt missing (CT2-03 deliverable)"); sys.exit(1)
if not os.path.exists(sc):
    print("FAIL: PHRASE_ALLOWLIST.sha256 sidecar missing"); sys.exit(1)
m_sc = re.fullmatch(r"([0-9a-f]{64})  PHRASE_ALLOWLIST\.txt\n?",
                    open(sc, encoding="utf-8").read())
if not m_sc:
    print("FAIL: PHRASE_ALLOWLIST.sha256 malformed (need exactly '<sha256>  PHRASE_ALLOWLIST.txt')")
    sys.exit(1)
actual = hashlib.sha256(open(ap, "rb").read()).hexdigest()
if actual != m_sc.group(1):
    print("FAIL: PHRASE_ALLOWLIST.txt was modified (%s != %s)" % (actual, m_sc.group(1)))
    sys.exit(1)
allow = {norm(l.strip()) for l in open(ap, encoding="utf-8") if l.strip() and not l.startswith("#")}

for t in targets:
    if not os.path.exists(t):
        print("FAIL: target missing:", t); sys.exit(1)
    raw = open(t, encoding="utf-8").read()
    # (0) token 筛子：目标内出现任一上游 harness 标识符即失败（原始文本，非规范化）
    for tok in TOKENS:
        if tok in raw:
            print("FAIL token sieve: %s contains upstream harness token %r" % (t, tok))
            bad = 1
    a = norm(raw)
    tg = {g for g in grams(a) if g not in allow}
    for srcp in sources:
        b = norm(open(srcp, encoding="utf-8", errors="replace").read())
        # (1) 长子串：>= 40 字符规范化公共子串
        for m in difflib.SequenceMatcher(None, a, b, autojunk=False).get_matching_blocks():
            if m.size >= 40:
                print("FAIL overlap %d chars: %s <-> %s :: %r"
                      % (m.size, t, os.path.basename(srcp), a[m.a:m.a + m.size]))
                bad = 1
        # (2) 8-token 滑窗：任何非白名单命中即失败（阈值 0，不是比率）
        shared = tg & grams(b)
        if shared:
            print("FAIL ngram %d shared 8-token windows: %s <-> %s :: %r"
                  % (len(shared), t, os.path.basename(srcp), sorted(shared)[0]))
            bad = 1
sys.exit(bad)          # ← 两种检测共用的唯一退出点
PY

# ============ 负例门：三个样本必须被内容检测拒绝 ============
# 2026-08-06：只断言「非零退出」不充分——grit pin/脏树/sidecar 缺失等环境性失败同样非零，
# 会让负例门在坏环境下空转通过。必须断言失败原因是三类内容检测之一。
for n in direct_process alias_call fragmented_copy; do
  cp "tests/compat-ledger/t4/_negative/$n.rs.txt" "$RUN_DIR/neg.rs"
  rc=0
  out=$(TARGET="$RUN_DIR/neg.rs" PHRASE_ALLOWLIST="$A_PA" PHRASE_SIDECAR="$A_SC" \
        EXPECTED_SNAP="${A_EXPECTED:-$SNAPD/tests/compat-ledger/t4/EXPECTED.txt}" sh "$A_CLEANROOM" 2>&1) \
    && { echo "FAIL: negative sample $n passed the clean-room gate"; exit 1; } || rc=$?
  printf '%s\n' "$out" | command grep -q -e '^FAIL token sieve:' -e '^FAIL overlap' -e '^FAIL ngram' \
    || { echo "FAIL: $n rejected for an environmental reason, not by content detection (rc=$rc):"; printf '%s\n' "$out"; exit 1; }
done
echo "OK: all three negative samples are rejected by content detection"

# ============ 篡改负例段：白名单被改则 sidecar 摘要先失败 ============
# R39 P1：**零工作树写入**——只篡改临时副本，`CLEANROOM.sh` 经 PHRASE_ALLOWLIST/PHRASE_SIDECAR
# 环境变量指向副本（here-doc 的 ap/sc 读取 os.environ 缺省原路径，见 ② 的白名单段）；
# 原文件全程不动，无 trap 窗口、无并发覆盖风险（GC-12）
PA="$A_PA"   # R73 P1：以快照副本为篡改基线（原文从工作树取，破坏 GC-15 的只读窗口）
SC="$A_SC"
[ -f "$PA" ] || { echo "FAIL: allowlist missing (CT2-03 deliverable)"; exit 1; }
[ -f "$SC" ] || { echo "FAIL: sidecar missing (CT2-03 deliverable)"; exit 1; }
# R43 P2：TDIR 挂在外层 RUN_DIR 下，不再安装会覆盖 CT302_GATE.sh 外层清理的内层 trap
TDIR="$RUN_DIR/tamper"; mkdir -p "$TDIR"
cp "$PA" "$TDIR/PHRASE_ALLOWLIST.txt"
cp "$SC" "$TDIR/PHRASE_ALLOWLIST.sha256"
printf '%s\n' "tampered entry added by negative gate" >> "$TDIR/PHRASE_ALLOWLIST.txt"
rc=0
out=$(TARGET=tests/command/t4_port_test.rs \
      PHRASE_ALLOWLIST="$TDIR/PHRASE_ALLOWLIST.txt" PHRASE_SIDECAR="$TDIR/PHRASE_ALLOWLIST.sha256" \
      EXPECTED_SNAP="${A_EXPECTED:-$SNAPD/tests/compat-ledger/t4/EXPECTED.txt}" sh "$A_CLEANROOM" 2>&1) \
  && { echo "FAIL: tampered PHRASE_ALLOWLIST.txt was not rejected"; exit 1; } || rc=$?
# 2026-08-06：断言失败原因确为 sidecar 摘要不符（与散文「先因 sidecar 摘要不符失败」对齐），
# 而不是任何环境性失败
printf '%s\n' "$out" | command grep -q '^FAIL: PHRASE_ALLOWLIST\.txt was modified' \
  || { echo "FAIL: rejection was not the sidecar mismatch (rc=$rc):"; printf '%s\n' "$out"; exit 1; }
echo "OK: tampered allowlist rejected by sha256 sidecar check"

# ============ ⑤ 变更集全量 token 筛子 ============
# 扫描对象 = ALLOWLIST.txt − 检测器自身 − 故意违规负例
# 2026-08-06：本门直接从已提交的 ALLOWLIST.txt 重新派生扫描集，不消费门 ① 的
# 门 ① 的中间产物——跨会话单独重跑本门时旧副本会让新增文件逃检（R42：已全部 run-scoped）
LC_ALL=C sort tests/compat-ledger/t4/ALLOWLIST.txt > "$RUN_DIR/ct302_allow5.txt"
command grep -v -e 'CLEANROOM\.sh$' -e 'CT302_GATE\.sh$' -e '/_negative/' "$RUN_DIR/ct302_allow5.txt" > "$RUN_DIR/ct302_scan.txt" || {
  rc=$?; [ "$rc" -eq 1 ] || { echo "ERROR: grep failed with $rc"; exit "$rc"; }; : > "$RUN_DIR/ct302_scan.txt"; }
[ -s "$RUN_DIR/ct302_scan.txt" ] || { echo "FAIL: nothing to scan after exclusions"; exit 1; }
# 模式由唯一 TOKENS 清单机械派生（CLEANROOM.sh 是该清单的唯一落盘处）
PAT=$(TOKSRC="$A_CLEANROOM" python3 - <<'PYPAT'
import os, re
src = open(os.environ["TOKSRC"], encoding="utf-8").read()
m = re.search(r"TOKENS\s*=\s*\[(.*?)\]", src, re.S)
assert m, "TOKENS list not found in CLEANROOM.sh"
toks = re.findall(r'"([^"]+)"', m.group(1))
assert len(toks) >= 6, "TOKENS list unexpectedly short"
print("|".join(re.escape(t) for t in toks))
PYPAT
) || { echo "FAIL: cannot derive token pattern from CLEANROOM.sh"; exit 1; }
if rg -n "$PAT" $(cat "$RUN_DIR/ct302_scan.txt"); then
  echo "FAIL: upstream harness text present in migrated tests"; exit 1
else
  rc=$?
  if [ "$rc" -ne 1 ]; then echo "ERROR: rg failed with exit $rc"; exit "$rc"; fi
  echo "OK: zero hits"
fi

# ============ 权威尾部（GC-15 ④ 消费窗口复验）============
# GC-15 ④：消费窗口结束——复验每个冻结锚点仍已提交、且工作树内容仍等于 ⓪ 段的快照
verify_frozen "$RUN_DIR" \
|| { echo "FAIL: a frozen anchor changed during the consumption window" >&2; exit 2; }
ct302_done=1        # 到这里才算真的跑完（见 `ct302_cleanup` 的 fail-closed 判据）
echo "OK: CT302_GATE.sh — all segments passed and every frozen anchor is unchanged"
