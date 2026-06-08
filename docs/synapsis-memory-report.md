# Synapsis Memory Report

## Executive Summary

TeamOlimpo 的 Synapsis 不是一个执行控制平面，也不是模型训练系统。它更像 agent-facing 的操作记忆层：把 session、observation、task、handoff、deliverable、knowledge chunk 和摘要统一存进 SQLite/FTS，然后通过 MCP 工具让 agent 主动检索、记录和交接。

Forge 已经有相近的数据基础：conversation messages、execution JSONL logs、transition logs、reviews、task comments、execution summaries、agent prompts。差距不在于没有数据，而在于这些数据目前按产品对象分散存储，agent 侧没有一个统一、低成本、可检索、可压缩、可写入的 memory protocol。

建议 Forge 学 TeamOlimpo 的 prompt 和 Synapsis 思路，但不要照搬它的文件式 handoff 和 prompt-only 约束。Forge 的优势是强运行时约束：workflow engine、worktree、execution、review、merge、DB 版本控制。更好的方向是在这些强约束之上加一层 Forge-native Memory Layer，让 agent 能检索历史经验、记录关键观察、产出结构化 handoff，并在下一次 claim/launch/follow-up 时自动得到相关上下文包。

这里的“提高 agent 训练效率”更准确地说是提高 agent 的操作学习效率：减少重复读上下文、复用历史失败/修复经验、让 reviewer/后续 agent 更快理解前序工作，而不是对模型权重做训练。

## TeamOlimpo/Synapsis Takeaways

### 1. Memory is an agent-facing workflow, not just storage

Synapsis 的关键不是 SQLite，而是 prompt 强制 agent 使用它：

- 会话开始：`synapsis_session(act="init")`
- 检索上下文：`synapsis_search(query, scope="auto", l=1, n=5)`（默认 layer 1，按需升层；或传 `tk` 让预算决定层级）
- 记录观察：`synapsis_session(act="observe")`
- 跟踪任务：`synapsis_task(act="create"|"update"|"log")`
- 创建交接：`synapsis_hf(act="new")`
- 注册交付物：`d_set` / `d_get`

这使 memory 成为 agent 工作流的一部分，而不是 UI 里的历史记录。

### 2. Handoff is the unit of learning

TeamOlimpo 的 handoff 要求每个 worker 输出状态、偏差、引用、质量分和下一步。这比普通 execution summary 更有学习价值，因为它告诉下一位 agent：

- 任务做到了哪里
- 哪些结论可复用
- 哪些地方失败或不确定
- 哪些文件/交付物是可信引用
- 后续应该继续、重试、升级还是停止

Forge 现在有 execution summary、review、logs、comments，但还没有一个 mandatory handoff object。

### 3. Progressive disclosure keeps context cheap

Synapsis 的 `l=1/2/3` 分层很实用：

- Layer 1：标题、计数、摘要
- Layer 2：关键片段和上下文
- Layer 3：全文

另外两个值得直接照抄的细节：`search` 的 `tk` 参数把 token 预算自动映射到 layer（<200 → l=1，≤1000 → l=2，否则 l=3），调用方只需声明预算；每条 observation 还记录 `tokens_discovery`/`tokens_read`/`token_savings`，让"省了多少上下文"可度量。这两点是后文 token-budgeted context pack 的现成 prior art。

Forge 当前 logs 和 conversations 可以读取，但还缺少“先给摘要，再按需展开”的 agent retrieval contract。

### 4. Prompt contracts matter

TeamOlimpo 的 Poros/Efesto prompts 值得借鉴：

- brief 必须有 Objective、Constraints、Acceptance criteria
- 多 agent/多步骤任务先 spec/plan，再执行
- worker failure 必须分类：transient、worker error、structural code bug、design bug、systemic
- shell、search、handoff、task tracking 都有明确工具优先级
- “handoff is the next brief”：上一位 agent 的交接直接成为下一位 agent 的输入

Forge 可以把这套 contract 融入 agent prompt templates，而不是只依赖用户手写任务描述。

## Forge Current State

Forge 已经具备 memory 的原材料：

- `conversation` / `conversation_message`：project chat 和 assistant turn history
- `execution`：prompt、summary、agent session id、parent execution chain
- JSONL execution logs：统一的 agent/process 输出 envelope
- `transition_log`：workflow 状态变化、trigger、hook results
- `review`：CI/auditor/human review 结果
- `task_comment` / `task_media`：人类反馈和附件
- `agent.prompt_template`：可注入 agent dispatch context

这些对象目前服务于 UI、审计、恢复和执行生命周期。它们还没有组成一个 agent 可主动查询的长期记忆层。

## Gap Analysis

### Gap 1: Memory is passive

Forge 保存了大量事实，但 agent 不一定知道如何检索它们。当前更像“事后可查看”，不是“执行前自动召回”。

Needed:

- 在 task claim、manual launch、follow-up 前自动构造 context pack
- 暴露 MCP 工具给 agent 主动检索项目历史
- 让 agent 在关键节点写 observation/decision/handoff

### Gap 2: Memory is fragmented by product table

同一条经验可能分散在 execution logs、review failure、transition hook、task comment 和 conversation 中。agent 需要一个统一入口，而不是理解 Forge 内部表结构。

Needed:

- `forge_memory_search`
- `forge_memory_get`
- `forge_memory_observe`
- `forge_handoff_create`
- `forge_context_pack`

### Gap 3: Summaries are too shallow for reuse

`execution.summary` 适合列表展示，但不够表达“为什么这么做、踩过什么坑、哪些检查通过、后续如何接手”。

Needed:

- execution completion summary
- structured handoff
- failure/deviation summary
- reusable lesson
- review verdict summary

These should be separate memory kinds instead of overloading one text field.

### Gap 4: No first-class quality score or confidence

TeamOlimpo 的 handoff 里真正 machine-readable 的字段只有 `st`（done/fail/hold/kill）、`prio` 和 `devi` deviation block —— Poros 的路由决策（继续、重试、升级）建立在这三个字段上。`quality_score`（1-5 自评）只存在于 SOP 文档约定中，并不是 `synapsis_hf` 的参数，也不是 `hf` 表的列；`confidence`（CONFIRMED/PARTIALLY_CONFIRMED/UNCONFIRMED）挂在 wiki 页面而不是 handoff 本身。换句话说，TeamOlimpo 自己也没把质量信号做成一等公民 —— 这正是 Forge 应该补上的：把这些约定落成结构化字段，而不是埋在 body 文本里。

Needed:

- handoff status: `done | fail | hold`（TeamOlimpo 还有 orchestrator 专用的 `kill`；Forge 的取消语义已由 `execution.status = cancelled` 覆盖，不需要复制）
- confidence: `confirmed | partial | unconfirmed`
- quality score: 1-5
- deviation category
- references to files, executions, reviews, tasks

### Gap 5: No memory consolidation loop

Long logs and long chats decay in usefulness unless compressed into stable summaries.

这一块 Synapsis 有现成的 prior art：`consolidate` MCP 工具、`session(act="compress")`、observation/task/event 上的 `compression_level` 字段、分级 `summaries` 表（level 1/2），以及 self-healing 检查（DB health score + orphan task 检测）。Forge 不必照搬实现，但"压缩分级 + 健康分 + 孤儿检测"这三个机制值得直接借鉴。

Needed:

- background summarization job
- stale/duplicate memory detection (cf. Synapsis health score + orphan detection)
- task-level final summary
- project-level lessons index
- review failure pattern summaries

## Proposed Forge Memory Layer

### Core concept

Add a Forge-native memory layer that indexes existing Forge events and lets agents write structured memories. It should not replace conversation, execution, review, or transition tables. It should sit above them as a normalized retrieval and learning layer.

### Suggested memory kinds

- `observation`: factual note discovered during work
- `decision`: design or implementation decision
- `handoff`: completion/failure/hold report from an agent
- `failure`: reusable failure pattern or deviation
- `review_result`: CI/auditor/human review finding
- `execution_summary`: concise turn summary
- `artifact`: file, diff, media, PR, or deliverable reference
- `lesson`: generalized reusable guidance
- `context_pack`: generated bundle used to launch an execution

### Suggested schema

```sql
CREATE TABLE memory_item (
    -- Explicit rowid alias: required for a stable FTS5 external-content mapping.
    -- A TEXT-PK table's implicit rowid can be renumbered by VACUUM, which would
    -- silently corrupt the FTS index.
    row_id INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,                -- app-generated UUID v4, like every other Forge table
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES task(id) ON DELETE SET NULL,
    execution_id TEXT REFERENCES execution(id) ON DELETE SET NULL,
    conversation_id TEXT REFERENCES conversation(id) ON DELETE SET NULL,
    source_type TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT,
    body TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    confidence TEXT,
    quality_score INTEGER,
    -- Polymorphic creator, same pattern as V012 unify_polymorphic_assignee.
    created_by_type TEXT,
    created_by_id TEXT,
    created_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE memory_item_fts USING fts5(
    title,
    summary,
    body,
    content='memory_item',
    content_rowid='row_id'
);

-- External-content FTS5 tables do not update themselves; sync triggers are
-- mandatory (Synapsis ships the same thing as _FTS_TRIGGERS_SQL).
CREATE TRIGGER memory_item_ai AFTER INSERT ON memory_item BEGIN
    INSERT INTO memory_item_fts(rowid, title, summary, body)
    VALUES (new.row_id, new.title, new.summary, new.body);
END;
CREATE TRIGGER memory_item_ad AFTER DELETE ON memory_item BEGIN
    INSERT INTO memory_item_fts(memory_item_fts, rowid, title, summary, body)
    VALUES ('delete', old.row_id, old.title, old.summary, old.body);
END;
```

Memory items are **append-only**: no `updated_at`, no `UPDATE` trigger, no `version` column. Immutability is what makes "what did the agent know when it acted?" answerable later — corrections are expressed as a new item that supersedes the old one (via `metadata_json`), not as an edit. If a mutable kind ever becomes necessary, it must follow the standard Forge optimistic-concurrency pattern (`version` column, `WHERE version = ?`, 409 on conflict).

This can start with FTS5 only. Vector search can come later if usage proves FTS is insufficient.

### Suggested MCP tools

Keep tool names Forge-native and typed:

- `forge_memory_search(project_id, query, scope, layer, limit)`
- `forge_memory_get(memory_id, layer)`
- `forge_memory_observe(project_id, task_id?, execution_id?, kind, title, body, metadata?)`
- `forge_handoff_create(task_id, execution_id, status, summary, references, deviations, quality_score)`
- `forge_context_pack(task_id, purpose, token_budget)`

These should call the same service layer as REST routes, like the current Forge MCP tools.

## Prompt Design Recommendations

### Add a default agent work contract

Every Forge coding agent prompt should include:

1. Restate the objective.
2. Identify constraints and acceptance criteria.
3. Call memory/context retrieval before changing code when a task has history.
4. Write observations only for durable facts, decisions, blockers, and reusable findings.
5. Produce a handoff before marking work complete.
6. Classify failures instead of burying them in logs.

### Standard handoff format

```markdown
## Summary

What changed, what was verified, and current status.

## Deliverables

- Files, commits, PRs, screenshots, logs, or docs produced.

## Key Findings

- Reusable facts or decisions.

## Verification

- Commands run and results.

## Deviations

- Missing input, blocked step, failed test, uncertainty, or workaround.

## Next Step

- What the next agent or human should do.
```

### Failure taxonomy

Forge should standardize failure labels:

- `transient`: timeout, network, temporary external service issue
- `input_missing`: task lacks required details
- `environment`: dependency, auth, daemon, workspace, path, permissions
- `code_bug`: implementation defect
- `design_gap`: unclear or contradictory system behavior
- `review_failed`: CI/auditor/human review rejection
- `systemic`: recurring pattern that needs product work

This makes memory searchable and trainable as operational feedback.

## Implementation Roadmap

### Phase 0: Prompt-only MVP

No migration. Improve agent prompt templates and execution instructions:

- Objective / Constraints / Acceptance criteria template
- mandatory final handoff in execution summary
- failure classification instructions
- reviewer prompt consumes previous handoff

This is cheap and immediately testable.

### Phase 1: Read-only memory index

Create `memory_item` as a normalized index over existing Forge data:

- execution summaries
- reviews
- transition logs
- task comments
- conversations

Expose `forge_memory_search` and `forge_memory_get`. This gives agents a unified memory interface without changing execution semantics.

Phase 1 落地清单（per CLAUDE.md conventions）：新增编号 migration（`V0NN__memory_item.sql`，含 FTS triggers）、MCP tool 名是 public surface 要在 CHANGELOG `Unreleased` 记录、`docs/api.md` 同步更新。Tools 走与现有 MCP tools 相同的 service layer，不依赖 `api` crate。

### Phase 2: Writeable observations and handoffs

Add `forge_memory_observe` and `forge_handoff_create`.

Start by making handoff optional but strongly encouraged. Once stable, make it part of agent completion policy for managed executions.

### Phase 3: Context packs

Before dispatch, Forge generates a bounded context pack:

- task objective and current state
- relevant prior memories
- latest execution/review handoff
- similar failure patterns
- project conventions and agent prompt template

The context pack should be persisted as a `memory_item` so later analysis can answer: “what did the agent know when it acted?”

### Phase 4: Consolidation and learning loops

Add scheduled summaries:

- per-task final memory
- per-project weekly lessons
- repeated failure clusters
- agent-specific quality feedback

This is where the “training efficiency” effect compounds: not by changing model weights, but by making every future run start with better distilled context.

## Product Impact

Expected benefits:

- Less repeated context gathering by agents
- Better handoffs between coder/reviewer/merge fixer
- Faster debugging of repeated CI/workspace failures
- More reliable follow-up executions
- Better auditability of what the agent knew and why it acted
- Higher quality agent prompts through observed failure patterns

Risks:

- Memory noise: agents may write too much low-value data
- Prompt injection: logs and user content become retrievable context
- Stale guidance: old lessons may conflict with new architecture
- Cross-project leakage: memory must be scoped carefully
- Storage growth: JSONL and summaries need retention policy

Required guardrails:

- project/task scoping on every memory query
- source attribution on every memory item
- confidence and freshness indicators
- token-budgeted context packs
- ability to exclude raw untrusted log payloads from automatic injection

## Recommendation

Adopt the Synapsis idea, not the Synapsis architecture.

Forge should keep its existing execution/workflow backbone and add a first-class memory layer that:

1. indexes existing product history,
2. gives agents a small MCP retrieval/write API,
3. introduces structured handoffs,
4. generates bounded context packs before execution,
5. consolidates repeated experience into durable lessons.

The best first step is Phase 0 plus a small Phase 1 spike: add prompt/handoff conventions now, then build a read-only memory index over execution summaries, reviews, comments, transition logs, and conversations. That will quickly show whether unified retrieval improves agent performance before committing to writable observations or summarization jobs.
