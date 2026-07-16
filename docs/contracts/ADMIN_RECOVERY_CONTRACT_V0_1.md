# Admin Recovery Contract v0.1

```text
Status: CURRENT
Architecture: svc-workflow v0.3.1
Slice: PR 5
Authority: PostgreSQL immutable facts plus rebuildable instance projection
```

本合同冻结 PR 5 的紧急恢复边界。它只提供受限的操作性修复命令，不建立第二套状态源，
也不把 Domain Owner、creator、current assignee 或 SERVICE Principal 隐式提升为管理员。

## 1. 管理员权威来源

actor 必须同时满足：

```text
Principal 存在
Principal.enabled = true
Principal.type in {HUMAN, AGENT}
实例 Domain 上存在 enabled domain_role_bindings
role_key = WORKFLOW_ADMIN
```

同一 Domain 可有多名 `WORKFLOW_ADMIN`。`DOMAIN_OWNER`、creator、current assignee 不继承该
权限；`SERVICE` 即使持有 binding 也拒绝。Domain disabled 不阻断紧急恢复。

存在但越权的实例请求与不存在的实例请求统一返回权限拒绝，避免管理端点成为实例存在性
探针。disabled/type 检查在读取实例前完成；Domain binding 在实例锁内重新校验并加共享锁。

Principal 不存在时，在 Receipt 边界前返回 `PrincipalNotFound`，不写 Receipt、AttemptAudit
或 SecurityAudit。这是现有 Principal 外键和上游认证边界的明确例外；存在但 disabled、类型
不允许或没有 binding 时，创建失败 Receipt，并写 CommandAttemptAudit 与 SecurityAudit。
disabled 与 SERVICE 分别稳定返回 `PrincipalDisabled`、`PrincipalTypeNotAllowed`；当前管理员
binding 缺失或实例不存在统一返回 `PermissionDenied`。

## 2. 通用幂等与事务规则

两条命令都使用：

```text
workflow_command_receipts
(principalId, idempotencyKey) 唯一键
JCS request envelope + SHA-256 requestHash
PROCESSING -> COMPLETED
成功与确定性失败稳定重放
相同 key 不同 hash 返回 IdempotencyConflict 并写 AttemptAudit
已提交的 PROCESSING 不被接管
```

Receipt acquire 之后，`Owned`、成功/失败 replay、hash conflict 和 PROCESSING 每个分支都必须
先锁 requested Instance（override 还锁 DefinitionVersion），再重新检查 actor 与当前
`WORKFLOW_ADMIN` binding。权限已经撤销时不得返回旧 response、commandId、原 request hash 或
PROCESSING 状态；既有 Receipt 不修改，只写不含 Receipt 元数据的 AttemptAudit/SecurityAudit。
`IdempotencyConflict` 对外是无原 commandId/hash 的不透明冲突，避免跨实例复用 key 泄漏。

固定锁序是：

```text
Receipt -> WorkflowInstance -> DefinitionVersion
```

确定性失败完成 Receipt，不留下部分事实或投影更新；数据库/审计/Event 等基础设施失败回滚
整个事务，包括新 Receipt。SecurityAudit 不保存 reason 正文、Context/Submission payload、
凭证或其他敏感数据。

## 3. Before snapshot digest v1

两条命令都在 Instance lock 内构造：

```json
{
  "schemaVersion": "WORKFLOW_INSTANCE_BEFORE_SNAPSHOT_V1",
  "workflowInstanceId": "lowercase-uuid",
  "domainId": "lowercase-uuid",
  "definitionVersionId": "lowercase-uuid",
  "createdByPrincipalId": "lowercase-uuid",
  "currentContextRevisionId": "lowercase-uuid-or-null",
  "currentNodeVisitId": "lowercase-uuid-or-null",
  "workflowStateVersion": 1
}
```

`beforeSnapshotDigest = SHA-256(JCS(document))`，字段名和 schemaVersion 固定。可选
`expectedBeforeSnapshotDigest` 必须是 64 位小写十六进制并在锁内比较；不匹配是可重放的
确定性失败。响应、SecurityAudit 和 override EventData 只保存 digest，不复制完整快照。

## 4. REBUILD_PROJECTION

输入固定为：

```text
actor / workflowInstanceId / idempotencyKey / commandSchemaVersion
optional expectedBeforeSnapshotDigest
```

它不要求 `expectedWorkflowStateVersion`，因为该字段本身可能损坏。命令从四类不可变事实
重新计算：

```text
currentContextRevisionId
currentNodeVisitId
workflowStateVersion
```

重建前必须 fail-closed 验证：

```text
Context revision 从 1 连续，previousRevisionId 紧邻，payload digest 正确
Visit 属于实例和固定 DefinitionVersion，进入 Transition 关系正确
Submission 的 source Visit / Context / Transition 关系和 payload digest 正确
Event sequence 从 1 连续，old/new version 与 sequence 一致
每个 Event 属于实例，引用的 Context / Visit / Submission 存在并属于本实例
EventData digest 正确，event type / nullable field matrix 合法
所有 Context / Visit / Submission fact 都被 Event sequence 覆盖
```

重放器是逐 Event 状态机：每一步维护 current Context、current Visit 和 state version；create /
import 必须引入 revision 1 与 initial Visit，revise 必须从当前 Context 前进一版，transition /
combined/admin 必须从当时 current Visit 出发并逐字段匹配 Definition、Context、Visit、Submission
及 payload digest。所有事实必须由唯一且正确的 Event 引入，最后不得指回旧 Context。

零事实、sequence 缝隙、孤立事实、未知 Event 或字段矩阵错误均拒绝。重建在 Instance 后锁住
其 DefinitionVersion，任何已知 lifecycle 状态都可重建。重建只允许更新 Instance
三个投影字段和 `updated_at`；不得新增、修改或删除 Context、Visit、Submission、Event，
不得增加 state version，也不得创建恢复 Event。

成功响应包含：

```text
commandId / workflowInstanceId
beforeProjection / afterProjection
beforeSnapshotDigest / changed / replayed
```

成功 Receipt、投影更新和 `REBUILD_PROJECTION_COMMITTED` SecurityAudit 同事务提交。

## 5. Event canonical names 与兼容别名

writer 只产生现有 canonical storage values：

```text
INSTANCE_CREATED
CONTEXT_REVISED
WORKFLOW_TRANSITION_COMMITTED
WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED
ADMIN_EMERGENCY_OVERRIDE_COMMITTED
```

rebuild reader 额外兼容架构别名：

```text
WORKFLOW_INSTANCE_CREATED -> INSTANCE_CREATED
WORKFLOW_CONTEXT_REVISED -> CONTEXT_REVISED
WORKFLOW_INSTANCE_IMPORTED (PR 6 initial/import shape)
```

别名不会被新 writer 写入，也不会产生第二套 Event。每个 canonical/alias 都使用显式字段矩阵；
未知名称一律 fail-closed。

## 6. ADMIN_EMERGENCY_OVERRIDE

输入固定为：

```text
actor / workflowInstanceId / idempotencyKey / commandSchemaVersion
expectedWorkflowStateVersion
operation = MOVE_TO_NODE | TERMINATE_INSTANCE
targetNodeId
reason (原值 1..2000 个可打印字符，禁止首尾空白)
optional expectedBeforeSnapshotDigest
relatedReferences (最多 20 项；type 1..128 bytes，id 1..256 bytes)
```

`PUBLISHED`、`DEPRECATED`、`REVOKED` DefinitionVersion 均允许；防御性 DRAFT 拒绝。target
必须属于实例固定的 DefinitionVersion。

任何写入前必须完整重放不可变事实，并证明 locked Instance 的三个 projection 字段与重放
结果完全一致；不一致是完成失败 Receipt 并写审计的确定性失败，不得创建 Visit/Event。
`workflowStateVersion + 1` 与 per-node `visitNumber + 1` 使用 checked arithmetic，溢出 fail-closed。
creator/fixed assignee Principal、Domain Owner binding 与最终 Principal 都以共享锁读取并检查 enabled。

操作矩阵：

| operation | target | 新 Visit assignee | Event effect |
|---|---|---|---|
| `MOVE_TO_NODE` | 非 TERMINAL | 按 enabled creator / Domain Owner / fixed Principal 正常解析 | `ADVANCE` |
| `TERMINATE_INSTANCE` | TERMINAL | `NULL` | `TERMINATE` |

每次成功都创建新的 Visit，`visitNumber = target Node 历史最大值 + 1`；即使 target 等于当前
Node 也不复用旧 Visit。新 Visit 的 `enteredByTransitionId = NULL`，不修改旧 Visit，不创建
Submission，不创建 Context Revision。Instance 保持 current Context，current Visit 切换到新
Visit，state version 恰好加 1。

成功必须恰好创建一条 `ADMIN_EMERGENCY_OVERRIDE_COMMITTED` Event：

```text
sourceNodeVisitId = old current Visit
targetNodeVisitId = new Visit
contextRevisionId = locked current Context
submissionId = NULL
old/new state version 与 eventSequence 一致
EventData = operation / bounded reason / bounded relatedReferences / beforeSnapshotDigest
```

Visit、Instance projection、Event、Receipt 和 `ADMIN_EMERGENCY_OVERRIDE_COMMITTED`
SecurityAudit 在一个事务中提交。

PR 4 timeline 对 Full scope 返回完整 admin EventData；HistoricalParticipant 仍可看到 terminal
outcome 的公开 Event 骨架，但 `event_data` 与 `event_data_digest` 必须为 `NULL`，不得暴露 reason
或 relatedReferences。

## 7. Terminal nullable 与历史兼容

规划阶段称为“0009”的语义迁移因仓库已有不可变
`0009_add_instance_external_reference.sql`，实际顺延为：

```text
0010_terminal_assignee_nullable.sql
```

它使 `workflow_node_definitions.assignee_ref_type` 与
`workflow_node_visits.assignee_principal_id` 可空，并对新行强制：

```text
TERMINAL Definition: assignee_ref_type = NULL, fixed_principal_id = NULL
non-TERMINAL Definition: assignee_ref_type 非空且 shape 合法
TERMINAL Visit: assignee_principal_id = NULL
non-TERMINAL Visit: assignee_principal_id 非空
Visit.node 的 definitionVersionId = Visit.instance 的固定 definitionVersionId
```

Definition check 以 `NOT VALID` 安装，因此既有 published Terminal definition/Visit 不回写、
不改变历史 digest。读取 legacy 非空 Terminal assignee 时，它只作为历史存储占位：不得授予
current-assignee visibility、assigned work 或命令权限。所有新 Terminal Visit 始终写 `NULL`；
查询 DTO 用 `Option<Uuid>` 表示，不把 canonical null 当成数据缺失。
