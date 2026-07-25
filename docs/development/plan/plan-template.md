# Libra 计划模板

本文是 `docs/development/plan/` 下新建计划的标准模板。新计划应复制本文件结构，替换 `<...>` 占位符，并删除不适用的说明性文字；强制章节不得删除，不适用时写 `N/A` 和原因。

## 使用规则

- 日期计划命名为 `plan-YYYYMMDD.md`，用于可执行的实现、迁移、发布或文档收敛任务。
- 长期能力只进入 `plan-long.md`。日期计划可以链接长期能力编号，但不得把长期路线图复制成重复任务表。
- 每个计划必须以当前 checkout 的源码、测试、用户文档和兼容矩阵为事实基线。历史计划、截图、会议记录或竞品描述只能作为线索，不能作为已实现证据。
- 每个任务卡必须能交给 Agent 独立执行：范围明确、依赖明确、文件落点明确、验收标准明确、验证命令明确。
- 涉及公开命令、配置、schema、错误码、存储格式、网络协议、Agent 数据、迁移、权限或安全边界的计划，必须包含测试、文档、回滚和兼容处理。
- 若计划使用外部项目或竞品作为参照，必须 pin 具体 revision、文件路径和核对日期；不得把浮动 `main` 当作规范。
- 新增或修改测试 target 时，必须同步 `Cargo.toml`、`tests/INDEX.md`，以及必要的 `tests/compat/README.md`。
- 生产代码不得新增未解释的 `unwrap()`、`expect()` 或 `panic!()`；如确属不可失败逻辑，必须有 `// INVARIANT:` 注释并在任务验收中说明。

## 标题

`# <主题>计划（<YYYY-MM-DD>）`

## 文档职责

本文解决 `<问题/能力>`，目标是 `<可交付结果>`。

本文只规划任务，不宣称实现完成。落地时每个任务都必须先刷新源码锚点，再按任务卡验收。

### 适用范围

- `<包含的命令/模块/用户工作流>`
- `<包含的存储、schema、协议或 UI 表面>`
- `<包含的测试、文档、兼容矩阵或发布动作>`

### 非目标

- `<明确不做的能力>`
- `<延后到其它计划/RFC/ADR 的范围>`
- `<容易被误解但本计划不承诺的行为>`

### 成功定义

- `<用户或系统行为变化>`
- `<机器接口或数据状态变化>`
- `<文档、测试、发布证据>`
- `<何时可标记计划完成>`

## 事实基线

> 所有行号和源码锚点必须在开工当天刷新。过期锚点只能作为历史线索。

| 类别 | 当前事实 | 证据 |
|---|---|---|
| 代码入口 | `<src/...>` | `<file:line>` |
| 数据/状态 | `<SQLite table/ref/object/path>` | `<file:line>` |
| 用户命令 | `<libra ...>` | `<src/cli.rs:line>` |
| 机器输出 | `<--json/--machine/schema>` | `<file:line>` |
| 错误码 | `<LBR-...>` | `<src/utils/error.rs:line>` |
| 文档 | `<docs/...>` | `<file:line>` |
| 测试 | `<tests/...>` | `<target::test_fn>` |
| 外部参照 | `<repo@sha>` | `<path + date>` |

### 当前缺口

| ID | 缺口 | 影响 | 证据 | 计划动作 |
|---|---|---|---|---|
| GAP-01 | `<问题>` | `<用户/生产影响>` | `<file:line 或外部证据>` | `<任务 ID>` |

## 与其它计划的关系

| 计划/文档 | 关系 | 本计划处理 |
|---|---|---|
| `plan-long.md` | `<关联 LR/SB/UP 编号>` | `<链接、消费、更新状态或不触碰>` |
| `plan-YYYYMMDD.md` | `<前置/并行/替代/冲突>` | `<复用、不重做、迁移、关闭>` |
| `docs/development/...` | `<事实源或契约>` | `<同步方式>` |

## 评审结论与修订记录

计划成稿前必须从以下维度做一次自审；如果有阻断项，先修计划再开工。

| 维度 | 结论 | 修订动作 |
|---|---|---|
| 合理性 | `<目标是否值得做>` | `<调整>` |
| 可行性 | `<任务是否可拆、可交付>` | `<调整>` |
| 完整性 | `<测试/文档/迁移/回滚是否齐全>` | `<调整>` |
| 安全性 | `<权限、secret、路径、网络、模型输入>` | `<调整>` |
| 功能正确性 | `<状态机、边界条件、错误路径>` | `<调整>` |
| 接口兼容 | `<CLI/API/schema/JSON/错误码>` | `<调整>` |
| 数据流与控制流 | `<事务、CAS、幂等、并发>` | `<调整>` |
| 性能与容量 | `<热路径、复杂度、存储增长>` | `<调整>` |
| 可靠性与容错 | `<崩溃恢复、重试、资源释放>` | `<调整>` |
| 可维护性 | `<事实源、抽象边界、重复实现>` | `<调整>` |

## 已决议设计决策

实现时若需偏离本节，必须先修改计划并说明原因，不得在代码中静默改语义。

### ADR-<PREFIX>-01: <决策标题>

- **Status:** Accepted
- **Context:** `<为什么需要这个决策>`
- **Decision:** `<选定方案>`
- **Alternatives considered:** `<备选方案及拒绝理由>`
- **Consequences:** `<带来的约束、风险、后续工作>`
- **Revisit when:** `<何时应重审>`

## 全局工程约束

以下约束对本文所有任务生效。任务条目不再逐条重复，违反任一项即视为任务未完成。

- **GC-01 现状核实前置:** 每个任务开工前重新核对计划、相关开发文档、用户文档、当前代码和测试。如果已实现，则任务改为补测试、补文档、更新状态或关闭，不重复实现。
- **GC-02 单一事实源:** 状态机、schema、输出结构、权限策略、配置解析和共享 helper 必须有单一事实源。禁止 CLI、Web、Agent adapter 或测试 fixture 各自复制逻辑。
- **GC-03 Git 互操作与 Libra 扩展边界:** Git 兼容表面必须说明与 Git 的一致点和有意差异；Libra-only 表面必须说明替代工作流、用户影响和机器接口。
- **GC-04 输出与错误契约:** 用户可见错误使用稳定 `LBR-*` 码并同步 `docs/error-codes.md`。`--json`、`--machine`、退出码和人读输出必须分别验收。
- **GC-05 文档同步:** 命令或公开行为变化必须同步 `docs/commands/<cmd>.md`、`docs/commands/zh-CN/<cmd>.md`、相关 `docs/development/commands/*.md`、`COMPATIBILITY.md` 和测试索引。
- **GC-06 测试索引:** 新增或重命名 `--test` target 必须更新 `Cargo.toml` 与 `tests/INDEX.md`；新增 `tests/compat/*` 还必须更新 `tests/compat/README.md`。
- **GC-07 安全默认值:** 未满足认证、授权、路径归属、schema 版本、对象闭包、sandbox 或 secret redaction 前置时默认 fail-closed。任何 fail-open 必须有显式用户选择、日志和测试。
- **GC-08 原子性与恢复:** 修改 HEAD、refs、index、worktree registry、SQLite、D1/R2、对象库、Agent session/checkpoint 或发布状态时，必须定义事务边界、幂等键、崩溃窗口和回滚/前滚策略。
- **GC-09 并发与资源生命周期:** 锁顺序、CAS、lease、队列、子进程、连接池、临时目录和 watcher 必须有释放/恢复语义；测试不得依赖未隔离的全局状态。
- **GC-10 性能预算:** `status`、`diff`、`add`、`commit`、fetch/push、Agent hot path 和 Web/SSE 热路径不得引入无界扫描、无界内存或 N+1 网络/DB 调用。需要时写出数据规模和断言。
- **GC-11 生产 panic 禁止:** 生产路径不得新增裸 `unwrap()`、`expect()`、`panic!()`；必须用 `Result`、`anyhow::Context` 或领域错误返回可操作信息。
- **GC-12 精确暂存:** 提交前只 `libra add <相关路径>`，不得使用 `commit -a`。发现无关脏状态时保留并报告，不得清理、重置或混入提交。

## 执行检查必备需求（强制）

任一要求未满足，对应任务不得标记完成。

1. **开工前安全检查:** 运行 `libra status --short`，确认当前分支、工作区脏状态和目标文件是否已有无关改动。若目标文件已有未确认用户改动，先报告并避免覆盖。
2. **先核对后实现:** 刷新本任务相关源码锚点、文档锚点、测试 target 和外部参照 revision，再决定实现、补测、补文档、关闭或降级。
3. **每个任务三门验收:** 至少通过 `cargo +nightly fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、任务卡指定的 `source .env.test && cargo test ...`。不能用泛泛的 `cargo test` 替代指定用例。
4. **Codex review 闭环:** 实现和本地验收完成后进行代码 review；review 问题修复后重跑相关验收，直到 review 明确通过。
5. **文档与兼容同步:** 涉及公开行为的任务必须同步用户文档、开发文档、兼容矩阵、错误码、help/examples、测试索引。
6. **Libra-native 工作流:** 本仓库使用 Libra 工作流：`libra status`、`libra add <相关路径>`、`libra commit -s -m "<scope>: <summary>"`、`libra push origin main`。不要把仓库当普通 Git 仓库处理。
7. **版本与发布:** 若任务发布用户可见代码改动，按开工时实际版本号 patch +1，同步 `Cargo.toml`、`web/package.json`、`worker/package.json`，运行 release build，并记录安装/发布证据。纯文档计划可写 `N/A`，但必须说明原因。
8. **push 失败策略:** 非 fast-forward 需要 pull/merge 后重新验收再推；认证、权限、网络或服务端失败不 blind retry，记录原因，待下一次修复/发布窗口处理。
9. **内部服务错误:** AI provider、MCP、R2/D1、GitHub API、release/download 等暂时性错误不得直接把任务宣告完成。记录错误并在同一步重试；确定性代码或配置错误转为当前任务修复项。
10. **证据卫生:** 验收证据不得保存 secret、API key、token、PII、未脱敏 transcript、绝对私有路径或原始 tool payload。需要留存时只写 sanitized summary。

## 实施顺序

依赖边格式：`A -> B` 表示 A 必须先于 B。

- `<TASK-01> -> <TASK-02>`
- `<TASK-02> -> <TASK-03>`

### Phase 0: <基线冻结和消歧>

**目标:** `<本阶段目标>`

**进入条件:**

- `<前置条件>`

**退出条件:**

- `<阶段完成判据>`

### Phase 1: <实现第一个可发布切片>

**目标:** `<本阶段目标>`

**进入条件:**

- `<前置条件>`

**退出条件:**

- `<阶段完成判据>`

## 任务卡

任务 ID 使用稳定前缀，例如 `A0-01`、`DR-01`、`W1-02`、`P0-03`。编号被引用后不重排，废弃时保留并标记替代关系。

### Task <ID>: <任务标题>

**Description:** `<要做什么、为什么、现实影响。必须写清不做什么。>`

**Current evidence:**

| 事实 | 证据 |
|---|---|
| `<当前实现或缺口>` | `<file:line / test / external repo@sha>` |

**Acceptance criteria:**

- [ ] `<用户可见或系统行为判据>`
- [ ] `<机器输出/错误码/schema 判据>`
- [ ] `<失败路径/边界条件判据>`
- [ ] `<文档/兼容/索引同步判据>`

**Verification:**

- [ ] `<exact command>`
- [ ] `<exact command>`
- [ ] `<manual/sanitized evidence, if required>`

**Dependencies:** `<无 / Task ID 列表 / 外部前置>`

**Files likely touched:** `<src/...>, <tests/...>, <docs/...>`

**Docs and compatibility impact:** `<N/A 或具体文件>`

**Migration and rollback:** `<N/A 或 up/down/前滚/回滚策略>`

**Security and privacy:** `<N/A 或权限、secret、path、redaction、sandbox 约束>`

**Performance budget:** `<N/A 或数据规模、复杂度、wall-clock/benchmark 断言>`

**Estimated scope:** `<S/M/L/XL>`

**Release boundary:** `<独立发布 / 随 Phase N 发布 / 文档-only N/A>`

## 测试矩阵

| 类别 | 必须覆盖 | Target / command |
|---|---|---|
| 单元 | `<纯逻辑、parser、state machine>` | `<cargo test ...>` |
| 集成 | `<真实 CLI 工作流>` | `<cargo test --test ...>` |
| 兼容 | `<Git/CLI/schema/文档矩阵>` | `<cargo test --test compat_...>` |
| 迁移 | `<up/down、old/new binary、故障注入>` | `<cargo test ...>` |
| 安全 | `<path traversal、secret、authz、sandbox>` | `<cargo test ...>` |
| 性能 | `<规模与预算>` | `<criterion / wall-clock>` |
| live/gated | `<真实外部服务或 provider>` | `<feature/env gated command>` |

## 追溯表

| 任务 | 来源/证据 | Libra 落点 | 文档/兼容动作 | 指定测试 |
|---|---|---|---|---|
| `<ID>` | `<file:line / issue / repo@sha>` | `<src/tests/sql/docs>` | `<docs/commands, COMPATIBILITY, error-codes>` | `<target::test_fn>` |

## 里程碑验收与回滚

| 里程碑 | 完成条件 | 发布/证据 | 回滚或前滚 |
|---|---|---|---|
| M0 | `<基线冻结>` | `<commit/test/doc>` | `<N/A>` |
| M1 | `<首个可发布切片>` | `<version/test/review>` | `<rollback/forward fix>` |

### 故障恢复矩阵

| 故障点 | 可接受残留 | 恢复动作 | 禁止结果 |
|---|---|---|---|
| `<reservation 后、提交前>` | `<lease/临时对象>` | `<retry/abandon/doctor>` | `<双写/数据丢失/静默成功>` |

## 风险登记

| 风险 | 影响 | 缓解 | 任务 |
|---|---|---|---|
| `<风险>` | `<高/中/低 + 影响>` | `<测试/设计/门禁>` | `<ID>` |

## 性能与容量摘要

| 操作 | 单次成本 | 累积成本 | 预算/上限 | 验证 |
|---|---|---|---|---|
| `<操作>` | `<O(...)>` | `<O(...)>` | `<阈值>` | `<测试/benchmark>` |

## 兼容与文档收口

- [ ] `COMPATIBILITY.md` 已同步，或说明 `N/A`。
- [ ] `docs/commands/<cmd>.md` 已同步，或说明 `N/A`。
- [ ] `docs/commands/zh-CN/<cmd>.md` 已同步，或说明 `N/A`。
- [ ] `docs/development/commands/<cmd>.md` 已同步，或说明 `N/A`。
- [ ] `docs/error-codes.md` 已同步，或说明 `N/A`。
- [ ] `tests/INDEX.md` 已同步，或说明 `N/A`。
- [ ] `Cargo.toml` `[[test]]` 已同步，或说明 `N/A`。
- [ ] `plan-long.md` 日期计划索引或 LR 状态已同步，或说明 `N/A`。

## Codex review log

| Round | Scope | Result | Required fixes | Evidence |
|---|---|---|---|---|
| R1 | `<files/tasks>` | `<PASS / issues>` | `<fix IDs>` | `<test commands>` |

## 非目标与延后项

| ID | 延后内容 | 原因 | 重启条件 | 承接位置 |
|---|---|---|---|---|
| DEFER-01 | `<内容>` | `<原因>` | `<何时重启>` | `<plan/RFC/ADR>` |

## 完成判据

计划只有在以下条件全部满足后才能标记完成：

- [ ] 所有非延后任务的 acceptance criteria 已满足。
- [ ] 所有任务的 Verification 命令已运行并记录结果。
- [ ] 必要的 docs/compat/error-code/test-index 更新已完成。
- [ ] 必要的 migration、rollback、failure-recovery 验证已完成。
- [ ] Codex review 已通过，或 residual risk 已明确记录并被接受。
- [ ] 如有发布要求，版本、构建、安装、提交、推送和发布证据已完成。
- [ ] `plan-long.md` 相关状态或日期计划索引已同步，或明确 `N/A`。
