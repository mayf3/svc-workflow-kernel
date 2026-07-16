# Definition Service Contract v0.1

```text
Status: IMPLEMENTATION_CONTRACT
Version: v0.1
Architecture: SVC_WORKFLOW_ARCHITECTURE_V0_3_1 (ARCHITECTURE_FROZEN)
PR: 2 — Definition & Immutable Version Publishing Service
```

> 本文档记录 PR 2 的实施合同，包括 Definition/Version 生命周期、图校验规则、Canonical Document 与 Digest 算法、权限边界和事务设计。

---

## 1. Definition / Version 生命周期

```text
DRAFT ──→ PUBLISHED ──→ DEPRECATED ──→ REVOKED
                  \──────────────→ REVOKED
```

### DRAFT
- 可修改图结构（Node、Transition、Context Schema）
- 不能用于创建 Workflow Instance

### PUBLISHED
- 图结构和业务字段冻结，不可修改
- 可用于创建新 Workflow Instance
- 允许对已有 Instance 执行 Context 修改和 Transition

### DEPRECATED
- 禁止创建新 Instance
- 已有 Instance 可继续运行

### REVOKED
- 禁止创建新 Instance
- 禁止已有 Instance 修改 Context 或执行普通 Transition
- 只允许管理员紧急修复

### 生命周期门禁（数据库 Trigger）
- `DRAFT → PUBLISHED`：允许
- `PUBLISHED → DEPRECATED`：允许
- `PUBLISHED → REVOKED`：允许
- `DEPRECATED → REVOKED`：允许
- 其他状态转换：拒绝

### 生命周期操作者

| 操作 | 写入字段 |
|------|----------|
| Publish | `published_by_principal_id` |
| Deprecate | `deprecated_by_principal_id` |
| Revoke | `revoked_by_principal_id` |

字段值取自命令的 `actor_principal_id`，后续生命周期转换不得覆盖已有操作者。`get_version`、`lock_version` 和 `list_versions` 的读模型必须完整返回以上三个字段。

---

## 2. Draft Graph 替换语义

ReplaceDraftGraph 是一个原子命令，在同一事务中完成：

1. `SELECT ... FOR UPDATE` 锁定 Definition Version 行
2. 校验 `version_status == 'DRAFT'`
3. 删除旧的 `workflow_node_definitions` 和 `workflow_transition_definitions`（先删 Transition，再删 Node）
4. 写入新的 Node 和 Transition 记录
5. 更新 `context_schema`
6. 提交事务

约束：
- 只允许替换 DRAFT 版本的图
- PUBLISHED/DEPRECATED/REVOKED 版本的图不可修改
- 发布后子表记录通过数据库 Trigger 保护（见第 7 节）

`context_schema: Option<Value>` 使用三态 Patch 语义：

| 输入 | 语义 |
|------|------|
| `None` | 不更新，保留当前值 |
| `Some(Value::Null)` | 显式清空，数据库列写为 SQL `NULL` |
| `Some(object)` | 用新 Schema 替换当前值 |

---

## 3. 发布校验规则

### 3.1 节点
1. 至少存在两个节点
2. 恰好一个 `DRAFT` 节点
3. `DRAFT` 是唯一入口
4. 至少一个 `TERMINAL` 节点
5. `orderIndex` 在版本内唯一
6. `nodeKey` 在版本内唯一
7. Terminal Node 没有负责人
8. 非 Terminal Node 必须具有合法负责人引用

### 3.2 主干
1. 每个非终态 Node 恰好指定一个 `primaryAdvanceTransitionId`
2. Primary Transition 必须从当前 Node 发出
3. Primary 目标的 `orderIndex` 必须更高
4. Primary 主干无环
5. 从 Draft 沿 Primary Transition 最终到达 Terminal Node
6. 所有非终态节点都必须位于或可达主干
7. 所有节点从 Draft 可达

### 3.3 RETURN
RETURN Transition 必须：
- 目标为非终态节点
- 目标 orderIndex 小于源节点
- 不是 `primaryAdvanceTransitionId`

### 3.4 TERMINATE
异常终止 Transition 必须：
- 不是 `primaryAdvanceTransitionId`
- 目标为 Terminal Node

### 3.5 Transition 完整性
1. `transitionKey` 在版本内唯一
2. source/target Node 都属于同一 Version
3. 禁止自环
4. 每条 Transition 的 Submission Schema 合法 JSON Schema
5. Primary Transition ID 必须实际存在

### 3.6 负责人
支持三种 `assigneeRef`：
- `WORKFLOW_CREATOR` — 流程创建者
- `DOMAIN_OWNER` — 域拥有者
- `FIXED_PRINCIPAL` — 固定 Principal（必须提供存在的 Principal ID）

规则：
- `FIXED_PRINCIPAL` 必须提供固定 Principal ID
- 非 `FIXED_PRINCIPAL` 不得提供固定 Principal
- 固定 Principal 必须存在且启用
- Draft Node 必须是 `WORKFLOW_CREATOR`
- Terminal Node 无负责人

### 3.7 JSON Schema
- `contextSchema` 和每条 Transition 的 `submissionSchema` 必须是合法 JSON Schema
- 使用冻结的 dialect（当前为 Draft 2020-12）
- 编译器和 validator 能成功加载
- 发布校验必须递归检查 `$ref`、`$dynamicRef` 和 `$recursiveRef`；只允许以 `#` 开头的本地 Fragment 引用
- 禁止 HTTP(S)、`file://` 和相对路径等外部引用，避免网络或本地文件解析

---

## 4. Canonical Definition Document

Digest 输入覆盖完整发布内容：

```text
Definition Key（稳定业务键）
Version Number
JSON Schema dialect
validatorVersion
contextSchema（完整 JSON）
全部 NodeDefinition
全部 TransitionDefinition
负责人引用（assigneeRefType + fixedPrincipalId）
primaryAdvanceTransitionKey
Submission Schema（每条 Transition）
```

### 排除字段
```text
数据库生成 ID（node_id, transition_id 等）
createdAt / updatedAt
publishedAt / publishedBy
数据库行顺序
metadata（可选业务元数据不影响合约）
```

### Canonical Document 结构
```json
{
  "definition_key": "string",
  "version_number": 1,
  "json_schema_dialect": "https://json-schema.org/draft/2020-12/schema",
  "validator_version": "1",
  "context_schema": { ... },
  "nodes": [
    {
      "node_key": "draft",
      "display_name": "Draft",
      "order_index": 0,
      "node_type": "DRAFT",
      "assignee_ref_type": "WORKFLOW_CREATOR",
      "fixed_principal_id": null,
      "instructions": null,
      "primary_advance_transition_key": "advance-dev",
      "metadata": null
    }
  ],
  "transitions": [
    {
      "transition_key": "advance-dev",
      "display_name": "Advance to Dev",
      "source_node_key": "draft",
      "target_node_key": "dev_self_check",
      "transition_effect": "ADVANCE",
      "submission_schema": null,
      "metadata": null
    }
  ]
}
```

### 排序规则
- Nodes 按 `node_key` 字典序排序
- Transitions 按 `transition_key` 字典序排序
- JSON 输出顺序由 serde 字段声明顺序决定（alphabetic 排序）

---

## 5. Digest 算法

1. 构造 Canonical Definition Document（见第 4 节）
2. 使用 `jcs-canonicalize` crate（RFC 8785 JSON Canonicalization Scheme）
3. SHA-256
4. 输出 64 字符小写十六进制字符串

保证：
- 相同业务内容产生相同 Digest（无论 JSON key 顺序、Node 插入顺序）
- 任何业务变化产生不同 Digest
- 时间戳、数据库 ID 不影响 Digest

---

## 6. 权限边界

### v0 策略
PR 2 只允许 `DOMAIN_OWNER` 执行 Definition 管理操作：

```text
CreateDefinition
CreateDraftVersion
ReplaceDraftGraph
ValidateDraftVersion
PublishVersion
DeprecateVersion
RevokeVersion
```

以下四个读操作也必须校验调用者是目标 Definition 所属 Domain 的 `DOMAIN_OWNER`，未授权时不得返回 Schema、Instructions 或固定负责人等内容：

```text
GetDefinition
GetDefinitionVersion
ListDefinitionVersions
GetCompleteVersionGraph
```

`ValidateDraftVersion` 同样受 `DOMAIN_OWNER` 写权限约束。除 `CreateDefinition` 已有门禁外，`CreateDraftVersion`、`ReplaceDraftGraph`、`ValidateDraftVersion`、`PublishVersion`、`DeprecateVersion` 和 `RevokeVersion` 都必须拒绝已禁用 Domain。

权限校验流程：
1. Principal 必须存在且启用
2. Domain 必须存在且启用
3. Actor 在 Domain 上必须有 `role_key = 'DOMAIN_OWNER'` 的 `DomainRoleBinding`

未来可通过扩展 `DomainRoleBinding` 角色实现更细粒度的 Definition 管理权限，当前不在 PR 2 实现。

---

## 7. 事务与锁

### 锁模式
- `SELECT ... FOR UPDATE` 用于保护 Definition Version 行
- 事务内读取数据保持一致性

### 并发保护
- 创建 Draft Version：`version_number` 通过 `MAX(version_number) + 1` 计算，唯一约束 `(workflow_definition_id, version_number)` 防止重复
- ReplaceDraftGraph 与 Publish：通过行锁序列化，不会同时成功
- Publish 与 Revoke/Deprecate：通过行锁+状态校验防止非法状态
- Digest 计算基于锁内读取的一致数据

### 数据库第二层防线
`workflow_node_definitions` 和 `workflow_transition_definitions` 子表通过 `fn_check_definition_graph_immutable()` 触发器保护：
- 父版本非 DRAFT 时拒绝 INSERT/UPDATE/DELETE
- UPDATE 操作同时检查 OLD 和 NEW 的 `definition_version_id`，防止通过修改父版本 ID 逃逸

---

## 8. 错误模型

```text
PrincipalNotFound
PrincipalDisabled
DomainNotFound
DomainDisabled
PermissionDenied
DefinitionNotFound
DefinitionVersionNotFound
DefinitionKeyConflict
VersionNotDraft
InvalidLifecycleTransition
GraphValidationFailed(Vec<GraphValidationError>)
SchemaValidationFailed(String)
FixedPrincipalInvalid(String)
DigestFailure(String)
ConcurrentModification(String)
StorageError(String)
```

GraphValidationError 包含 `code`（机器可读）和 `message`（人类可读）。

PostgreSQL 错误必须映射为稳定的领域错误：

| 数据库条件 | 领域错误 |
|------------|----------|
| `23505` 且约束/消息含 `definition_key` | `DefinitionKeyConflict` |
| `23505` 且约束/消息含 `version_number` | `ConcurrentModification` |
| Trigger 消息含 `graph_immutable:` | `VersionNotDraft` |
| Trigger 消息含 `status_transition:` | `InvalidLifecycleTransition` |
| 其他数据库错误 | `StorageError(raw)` |

---

## 9. 关闭的 Medium：Graph 父版本移动逃逸

### 原始发现
`fn_check_definition_graph_immutable()` 只检查 `NEW.definition_version_id`，UPDATE 操作可通过同时修改 `definition_version_id` 将记录从 PUBLISHED 版本移动到 DRAFT 版本。

### 修复（Migration 0008）
更新后的 Trigger Function 同时检查：
- `OLD.definition_version_id` — 如果 OLD 父版本非 DRAFT 且与 NEW 不同，拒绝
- `NEW.definition_version_id` — 如果 NEW 父版本非 DRAFT，拒绝

### 新增测试
1. PUBLISHED → DRAFT 移动 Node 被拒绝 ✅
2. DRAFT → PUBLISHED 移动 Node 被拒绝 ✅（由 NEW 检查覆盖）
3. PUBLISHED → DRAFT 移动 Transition 被拒绝 ✅
4. DRAFT → PUBLISHED 移动 Transition 被拒绝 ✅（由 NEW 检查覆盖）

---

## 10. 明确不在 PR 2 实现

- HTTP API
- Workflow Instance 创建
- Context Revision 命令
- Submission
- Transition Engine (ADVANCE/RETURN/TERMINATE)
- assigned-to-me 查询
- 完整 CommandReceipt 幂等框架
- 管理员恢复
- Legacy 模板导入
- ADC 或 llm-todo 修改
- 服务部署
- 多 crate 拆分
- Subject、并行、Timer、Signal、Reassign
- 业务 Validator
