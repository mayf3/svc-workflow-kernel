# PostgreSQL Storage Contract v0.1

```text
Status: STORAGE_CONTRACT
Version: v0.1
Architecture: SVC_WORKFLOW_ARCHITECTURE_V0_3_1 (ARCHITECTURE_FROZEN)
```

> 本文档记录 `svc-workflow` PostgreSQL 存储骨架的表设计、约束体系和不可变规则。
> 它是 PR 1（仓储骨架与不可变核心事实表）的实施合同。
> 本文档不改变冻结架构的核心含义。

---

## 1. 表清单与所有权

所有表位于 `public` schema（推荐独立 `workflow` schema，本 PR 暂用 public）。

| # | 表名 | 分类 | 写入者 |
|---|------|------|--------|
| 1 | `principals` | 身份 | svc-workflow 独占 |
| 2 | `domains` | 域 | svc-workflow 独占 |
| 3 | `domain_role_bindings` | 域权限 | svc-workflow 独占 |
| 4 | `workflow_definitions` | 定义 | svc-workflow 独占 |
| 5 | `workflow_definition_versions` | 定义版本 | svc-workflow 独占 |
| 6 | `workflow_node_definitions` | 节点定义 | svc-workflow 独占 |
| 7 | `workflow_transition_definitions` | 转换定义 | svc-workflow 独占 |
| 8 | `workflow_instances` | 运行时 | svc-workflow 独占 |
| 9 | `workflow_context_revisions` | 运行时（不可变事实） | svc-workflow 独占 |
| 10 | `workflow_node_visits` | 运行时（不可变事实） | svc-workflow 独占 |
| 11 | `workflow_submissions` | 运行时（不可变事实） | svc-workflow 独占 |
| 12 | `workflow_events` | 运行时（不可变事实） | svc-workflow 独占 |
| 13 | `workflow_command_receipts` | 命令 | svc-workflow 独占 |
| 14 | `workflow_command_attempt_audits` | 审计 | svc-workflow 独占 |
| 15 | `workflow_security_audits` | 审计 | svc-workflow 独占 |

上层产品即使能连接同一 PostgreSQL 集群，也不得直接写入这些表。

---

## 2. Enum 类型

| PostgreSQL 类型名 | 值 |
|---|---|
| `principal_type` | `HUMAN`, `AGENT`, `SERVICE` |
| `definition_version_status` | `DRAFT`, `PUBLISHED`, `DEPRECATED`, `REVOKED` |
| `node_type` | `DRAFT`, `NORMAL`, `TERMINAL` |
| `assignee_ref_type` | `WORKFLOW_CREATOR`, `DOMAIN_OWNER`, `FIXED_PRINCIPAL` |
| `transition_effect` | `ADVANCE`, `RETURN`, `TERMINATE` |
| `receipt_status` | `PROCESSING`, `COMPLETED` |

---

## 3. 主键与复合唯一键

### 主键

所有实体使用 UUID 主键，由应用层生成（`gen_random_uuid()` 或 Rust 的 `Uuid::new_v4()`）。

### 复合唯一键

| 表 | 唯一键 | 说明 |
|---|---|---|
| `domain_role_bindings` | `(domain_id, principal_id, role_key)` | 同一域同一角色每人最多一个 binding |
| `domain_role_bindings` | `(domain_id, role_key)` WHERE `enabled=true AND role_key='DOMAIN_OWNER'` | 部分唯一索引：最多一个有效 Owner |
| `workflow_definitions` | `(domain_id, definition_key)` | 域内 definition_key 唯一 |
| `workflow_definition_versions` | `(workflow_definition_id, version_number)` | 定义内版本号唯一 |
| `workflow_node_definitions` | `(definition_version_id, node_key)` | 版本内 node_key 唯一 |
| `workflow_node_definitions` | `(definition_version_id, order_index)` | 版本内 order_index 唯一 |
| `workflow_transition_definitions` | `(definition_version_id, transition_key)` | 版本内 transition_key 唯一 |
| `workflow_context_revisions` | `(workflow_instance_id, revision_number)` | 实例内 revision_number 唯一 |
| `workflow_node_visits` | `(workflow_instance_id, node_id, visit_number)` | 实例内同一节点的 visit 编号唯一 |
| `workflow_submissions` | `(source_node_visit_id)` | 每个 visit 最多一个 committed submission |
| `workflow_events` | `(workflow_instance_id, event_sequence)` | 实例内 event_sequence 唯一 |
| `workflow_events` | `(command_id)` WHERE `command_id IS NOT NULL` | 每个 command 最多对应一个 event |
| `workflow_command_receipts` | `(principal_id, idempotency_key)` | 幂等键唯一 |

---

## 4. 复合外键

确保跨表引用属于同一 Workflow Instance。

| 源表 | 源列 | 目标表 | 目标列 | 延迟 |
|---|---|---|---|---|
| `workflow_submissions` | `(source_node_visit_id, workflow_instance_id)` | `workflow_node_visits` | `(node_visit_id, workflow_instance_id)` | DEFERRABLE INITIALLY DEFERRED |
| `workflow_submissions` | `(context_revision_id, workflow_instance_id)` | `workflow_context_revisions` | `(context_revision_id, workflow_instance_id)` | DEFERRABLE INITIALLY DEFERRED |
| `workflow_events` | `(source_node_visit_id, workflow_instance_id)` | `workflow_node_visits` | `(node_visit_id, workflow_instance_id)` | DEFERRABLE INITIALLY DEFERRED |
| `workflow_events` | `(target_node_visit_id, workflow_instance_id)` | `workflow_node_visits` | `(node_visit_id, workflow_instance_id)` | DEFERRABLE INITIALLY DEFERRED |
| `workflow_events` | `(context_revision_id, workflow_instance_id)` | `workflow_context_revisions` | `(context_revision_id, workflow_instance_id)` | DEFERRABLE INITIALLY DEFERRED |
| `workflow_events` | `(submission_id, workflow_instance_id)` | `workflow_submissions` | `(submission_id, workflow_instance_id)` | DEFERRABLE INITIALLY DEFERRED |
| `workflow_context_revisions` | `(previous_revision_id, workflow_instance_id)` | `workflow_context_revisions` | `(context_revision_id, workflow_instance_id)` | DEFERRABLE INITIALLY DEFERRED（自引用） |

### 循环外键（创建实例时）

```sql
workflow_instances (current_context_revision_id, workflow_instance_id)
  → workflow_context_revisions (context_revision_id, workflow_instance_id)
  DEFERRABLE INITIALLY DEFERRED

workflow_instances (current_node_visit_id, workflow_instance_id)
  → workflow_node_visits (node_visit_id, workflow_instance_id)
  DEFERRABLE INITIALLY DEFERRED
```

创建实例时，预生成 `instance_id`、`context_revision_id`、`node_visit_id` 并在同一事务中依次插入：
1. `workflow_instances`（引用尚不存在的 ctx 和 visit）
2. `workflow_context_revisions`（引用 instance_id）
3. `workflow_node_visits`（引用 instance_id）
4. Commit 时所有 FK 验证通过。

---

## 5. 不可变触发器

### 绝对不可变表（禁止 UPDATE 和 DELETE）

| 表 | 触发器 | 效果 |
|---|---|---|
| `workflow_context_revisions` | `trg_context_revisions_immutable` | UPDATE 和 DELETE 均拒绝 |
| `workflow_node_visits` | `trg_node_visits_immutable` | UPDATE 和 DELETE 均拒绝 |
| `workflow_submissions` | `trg_submissions_immutable` | UPDATE 和 DELETE 均拒绝 |
| `workflow_events` | `trg_events_immutable` | UPDATE 和 DELETE 均拒绝 |
| `workflow_command_receipts`（COMPLETED 后） | `trg_command_receipts_completed_immutable` | COMPLETED 后 UPDATE 和 DELETE 均拒绝 |

### 条件不可变表

**workflow_instances**：`trg_instance_immutable_fields` 阻止修改以下字段：
- `domain_id`
- `definition_version_id`
- `created_by_principal_id`
- `created_at`

其他投影字段（`current_context_revision_id`、`current_node_visit_id`、`workflow_state_version`）允许后续 Command Service 更新。

**workflow_definition_versions**：`trg_definition_version_immutable` 阻止 PUBLISHED/DEPRECATED/REVOKED 状态下修改业务字段：
- `definition_digest`
- `json_schema_dialect`
- `validator_version`
- `context_schema`
- `submission_schema`
- `metadata`

### 状态生命周期触发器

**Receipt 状态**：`trg_command_receipts_status_check` 只允许 `PROCESSING → COMPLETED`。

**Definition Version 状态**：`trg_definition_version_status_transition` 只允许：
- `DRAFT → PUBLISHED`
- `PUBLISHED → DEPRECATED`
- `PUBLISHED → REVOKED`
- `DEPRECATED → REVOKED`

禁止：
- `PUBLISHED → DRAFT`
- `DEPRECATED → PUBLISHED`
- `REVOKED → anything`
- `DEPRECATED → DRAFT`

**Completed 时间**：`trg_receipt_set_completed_at` 自动设置 `completed_at`。

**COMPLETED 必填字段**：`trg_receipt_check_completed_fields` 要求 `response_status` 非空。

---

## 6. 大小限制

| 列 | 限制 | DDL 检查 | 说明 |
|---|---|---|---|
| `workflow_context_revisions.payload` | ≤ 1 MiB | `CHECK (pg_column_size(payload) <= 1048576)` | pg_column_size 返回 JSONB 二进制存储大小 |
| `workflow_submissions.payload` | ≤ 1 MiB | `CHECK (pg_column_size(payload) <= 1048576)` | 同上 |
| `workflow_instances.metadata` | ≤ 64 KiB | `CHECK (metadata IS NULL OR pg_column_size(metadata) <= 65536)` | 同上 |
| `workflow_definitions.metadata` | ≤ 64 KiB | `CHECK (metadata IS NULL OR pg_column_size(metadata) <= 65536)` | 同上 |
| `workflow_definition_versions.metadata` | ≤ 64 KiB | `CHECK (metadata IS NULL OR pg_column_size(metadata) <= 65536)` | 同上 |
| `workflow_command_receipts.response_body` | ≤ 1 MiB | `CHECK (response_body IS NULL OR pg_column_size(response_body) <= 1048576)` | 同上 |
| `workflow_events.event_data` | ≤ 256 KiB | `CHECK (event_data IS NULL OR pg_column_size(event_data) <= 262144)` | 同上 |

### 关于 pg_column_size 的说明

`pg_column_size()` 计算的是 JSONB 二进制存储格式的大小，不是输入 JSON 字符串的大小。JSONB 二进制格式包含类型标签、键去重和压缩等开销，因此：
- 实际输入 JSON 字符串可能略大于或略小于 pg_column_size 的值
- 对于绝大多数业务数据，95 分位偏差在 ±10% 以内

**分层覆盖策略**：
- **数据库层**（DDL CHECK）：提供防御性硬限制，防止极端超大 payload 进入数据库
- **服务层**（Rust 验证）：后续 PR 在 Command Service 中实现精确的大小校验，在读取和解码前拒绝超限请求

---

## 7. Transaction 假设

所有工作流状态命令假设以下 PostgreSQL 配置：

```sql
SET transaction_isolation = 'read committed';
```

默认锁顺序：
1. `workflow_command_receipts`（幂等键）
2. `workflow_instances`（行锁）
3. 其他必要只读引用

所有命令事务使用 `ON CONFLICT DO NOTHING RETURNING` 进行乐观幂等插入。

DEFERRABLE INITIALLY DEFERRED 约束在事务提交时一次性验证，允许循环 FK 在单事务中满足。

---

## 8. DDL 覆盖 vs 后续 Command Service 覆盖

| 规则 | DDL 保证 | Command Service 保证 |
|---|---|---|
| Domain Owner 唯一（最多一个 enabled） | ✅ 部分唯一索引 | ✅ 管理事务校验"至少一个" |
| Definition 属于一个 Domain | ✅ FK + 唯一键 | ✅ 路由校验 |
| Definition Version 不可变（发布后） | ✅ 触发器 | ✅ 业务层校验 |
| Context Revision 编号唯一 | ✅ 唯一键 | ✅ |
| Context Revision 禁止跨 Instance 引用 | ✅ 复合 FK | ✅ |
| Context Revision 不可修改/删除 | ✅ 触发器 | ✅ |
| Node Visit 编号唯一 | ✅ 唯一键 | ✅ |
| Node Visit 不可修改/删除 | ✅ 触发器 | ✅ |
| 每个 Visit 最多一个 Submission | ✅ 唯一约束 | ✅ |
| Submission 不能混用其他 Instance | ✅ 复合 FK | ✅ |
| Event 实体引用不能跨 Instance | ✅ 复合 FK | ✅ |
| Event Sequence 唯一 | ✅ 唯一键 | ✅ |
| 一个 Command 最多一个 Event | ✅ 部分唯一索引 | ✅ |
| COMPLETED Receipt 不可修改/删除 | ✅ 触发器 | ✅ |
| Instance 不可变字段 | ✅ 触发器 | ✅ |
| 初始循环外键 | ✅ DEFERRABLE | ✅ 预生成 ID |
| Definition Version 状态回退禁止 | ✅ 触发器 | ✅ |
| 大小限制 | ✅ CHECK 约束 | ✅ 服务层精确校验 |
| Receipt PROCESSING→COMPLETED | ✅ 触发器 | ✅ 业务层校验 |
| 数据 digest 一致性 | ❌ | ✅ 服务层比较 |
| Schema 校验 | ❌ | ✅ Command Service |
| 负责人解析 | ❌ | ✅ Transition 引擎 |
| 幂等请求去重 | ❌ ON CONFLICT (部分) | ✅ 完整 ON CONFLICT 流程 |
| Context 和 Submission JSON Schema 校验 | ❌ | ✅ Command Service |
| Definition 图结构校验 | ❌ | ✅ Definition Service |

---

## 9. Migration 文件

| 文件 | 内容 |
|---|---|
| `0001_identity_domain.sql` | Enums（6 个）、principals、domains、domain_role_bindings |
| `0002_workflow_definition.sql` | workflow_definitions、workflow_definition_versions、workflow_node_definitions、workflow_transition_definitions |
| `0003_runtime.sql` | workflow_instances、workflow_context_revisions、workflow_node_visits、workflow_submissions |
| `0004_workflow_events.sql` | workflow_events |
| `0005_command_audit.sql` | workflow_command_receipts、workflow_command_attempt_audits、workflow_security_audits |
| `0006_triggers_constraints.sql` | 所有触发器（不可变保护、状态生命周期、大小检查）、deferred FK、剩余约束 |

---

## 10. 发布后 Definition 图不可变（审计修复）

| 表 | 触发器 | 效果 |
|---|---|---|
| `workflow_node_definitions` | `trg_node_definitions_graph_immutable` | INSERT/UPDATE/DELETE 仅当父版本为 DRAFT 时允许 |
| `workflow_transition_definitions` | `trg_transition_definitions_graph_immutable` | INSERT/UPDATE/DELETE 仅当父版本为 DRAFT 时允许 |

触发器函数 `fn_check_definition_graph_immutable()` 查询父级 `workflow_definition_versions.version_status`。如果父版本为 PUBLISHED、DEPRECATED 或 REVOKED，拒绝所有修改。

错误信息以 `graph_immutable:` 开头，便于测试断言。

## 11. 审计修复汇总

以下保护由 Migration `0007_definition_graph_immutability.sql` 添加：

### 11.1 Definition 子表保护（Blocker 修复）

见第 10 节。

### 11.2 Workflow Instance 额外不可变字段

`fn_check_instance_immutable_fields()`（重写在 0007 中）新增保护以下字段：

- `external_url` — 创建后不可修改
- `metadata` — 创建后不可修改

### 11.3 PROCESSING Receipt 身份字段保护

新触发器 `trg_receipt_identity_immutable`（0007 创建）保护 `workflow_command_receipts` 在任意状态下：

| 保护字段 | 说明 |
|---|---|
| `command_id` | 命令唯一标识 |
| `principal_id` | 发起者身份 |
| `idempotency_key` | 幂等键 |
| `command_type` | 命令类型 |
| `request_hash` | 请求哈希 |
| `created_at` | 创建时间 |

允许修改的字段：`receipt_status`, `response_status`, `response_body`, `response_digest`, `completed_at`。

### 11.4 Size 约束测试覆盖（7/7）

所有 7 个 size CHECK 约束现在都有测试覆盖：

| 约束名 | 表 | 列 | 限制 | 测试 |
|---|---|---|---|---|
| `chk_ctx_payload_size` | workflow_context_revisions | payload | ≤ 1 MiB | ✅ `test_context_payload_size_limit` |
| `chk_submission_payload_size` | workflow_submissions | payload | ≤ 1 MiB | ✅ `test_submission_payload_size_limit` |
| `chk_instance_metadata_size` | workflow_instances | metadata | ≤ 64 KiB | ✅ `test_instance_metadata_size_limit` |
| `chk_def_metadata_size` | workflow_definitions | metadata | ≤ 64 KiB | ✅ `test_definition_metadata_size_limit` |
| `chk_def_ver_metadata_size` | workflow_definition_versions | metadata | ≤ 64 KiB | ✅ `test_definition_version_metadata_size_limit` |
| `chk_receipt_response_size` | workflow_command_receipts | response_body | ≤ 1 MiB | ✅ `test_receipt_response_body_size_limit` |
| `chk_event_data_size` | workflow_events | event_data | ≤ 256 KiB | ✅ `test_event_data_size_limit` |

### 11.5 Deferred FK 当前策略

所有复合外键目前使用 `DEFERRABLE INITIALLY DEFERRED`。真正需要 deferral 的 FK：

1. `fk_instance_current_ctx` — Instance ↔ ContextRevision 循环引用
2. `fk_instance_current_visit` — Instance ↔ NodeVisit 循环引用
3. `fk_previous_revision` — ContextRevision 自引用循环
4. `fk_primary_advance_transition` — Node ↔ Transition 循环（同一版本内）

以下 FK 没有循环依赖，但当前保持 deferred 以统一开发模式：

- `fk_submission_visit_same_instance`
- `fk_submission_ctx_same_instance`
- `fk_event_source_visit_same_instance`
- `fk_event_target_visit_same_instance`
- `fk_event_ctx_same_instance`
- `fk_event_submission_same_instance`
- `fk_event_command`

**后续收窄点**：当 Command Service 稳定后，可将非循环 FK 改为 `NOT DEFERRABLE`，前提是不破坏现有事务流程。

## 12. 已知限制与后续任务

1. **大小限制**：DDL 中的 `pg_column_size` 检查是防御性硬限制。精确的输入字符串大小校验应在 Rust 服务层实现。
2. **幂等 ON CONFLICT 完整流程**：本 PR 已建立唯一约束保护，但完整的 `INSERT ... ON CONFLICT DO NOTHING RETURNING` 命令执行流程在后续 Command Service PR 实现。
3. **Domain Owner "至少一个"**：数据库保证"最多一个"，"至少一个"由后续 Domain 管理事务保证。
4. **Context Revision 单链**：第一条 Revision 的 `previous_revision_id = null` 及后续链规则由 Command Service 实现，数据库只保证同实例引用完整性。
5. **Definition 图结构校验**：发布前校验（恰好一个 Draft 入口、主干无环、Terminal 无出边等）在后续 Definition Service PR 实现。
6. **eventSchemaVersion 版本管理**：本 PR 存储字符串字段，版本模式由后续 PR 定义。
7. **没有全局 event cursor**：符合 v0.3.1 冻结决策。
8. **OperationalTelemetry**：本 PR 不落领域表，后续使用运行监控系统处理。
