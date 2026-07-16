# svc-workflow 实施契约勘误 v0.1

```text
Status: IMPLEMENTATION_CONTRACT
Version: v0.1
Architecture: SVC_WORKFLOW_ARCHITECTURE_V0_3_1 (ARCHITECTURE_FROZEN)
```

> 本文件仅记录已经确认的实施层契约，用于补齐 `SVC_WORKFLOW_ARCHITECTURE_V0_3_1.md` 未完全展开的实现细节。
> 本文件不改变已冻结的领域架构，也不引入并行、Timer、Signal、Reassign、Subject 或业务 Validator。
> 当本文件与架构文档冲突时，以架构文档为准。

---

## 1. CommandReceipt 幂等性（PostgreSQL 实现）

CommandReceipt 使用 PostgreSQL 的乐观插入实现幂等：

```sql
INSERT ... ON CONFLICT DO NOTHING RETURNING ...
```

插入失败后再执行：

```sql
SELECT ... FOR UPDATE
```

以读取已存在的 Receipt。

针对相同 `idempotencyKey`、不同 `requestHash` 的请求：

* 不修改原 Receipt；
* 写 `CommandAttemptAudit`；
* 返回 `409 Conflict`。

---

## 2. Transition-only 的 `Submission.contextRevisionId` 绑定

* Transition-only（无 Context 修改）场景下，`Submission.contextRevisionId` 由服务端在绑定锁内读取的 `currentContextRevisionId` 绑定，客户端不能自由指定。
* Context + Transition 场景下，绑定本事务内新建的 Revision。
* 客户端不能自由指定历史 Context Revision。

---

## 3. RETURN 引用完整性

RETURN 类操作涉及的对当前 Instance 的引用必须满足：

* `rootCauseNodeVisitId` 属于当前 Instance；
* `relatedSubmissionIds` 均属于当前 Instance；
* 引用必须在当前命令之前已经存在；
* 禁止跨 Instance 引用。

---

## 4. Event 字段矩阵

| eventType                                         | sourceVisit    | targetVisit        | contextRevision     | submission   |
| ------------------------------------------------- | -------------- | ------------------ | ------------------- | ------------ |
| WORKFLOW_INSTANCE_CREATED                         | null           | 初始 Draft Visit    | Revision #1         | null         |
| WORKFLOW_CONTEXT_REVISED                          | 当前 Visit      | 同一 Visit          | 新 Revision          | null         |
| WORKFLOW_TRANSITION_COMMITTED                     | 旧 Visit        | 新 Visit            | 当前 Revision        | 新 Submission |
| WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED | 旧 Draft Visit  | 新 Visit            | 新 Revision          | 新 Submission |
| WORKFLOW_INSTANCE_IMPORTED                        | null           | 导入 Visit          | 导入 Revision        | null         |
| ADMIN_EMERGENCY_OVERRIDE_COMMITTED                | 旧 Visit        | 新 Visit            | 命令完成后的当前 Revision | null         |

统一规则：

```text
targetNodeVisitId   = 命令完成后的 currentNodeVisitId
contextRevisionId   = 命令完成后的 currentContextRevisionId
```

---

## 5. Definition Version 状态门禁

```text
PUBLISHED:
  允许创建实例，已有实例继续

DEPRECATED:
  禁止创建新实例，已有实例继续

REVOKED:
  禁止创建、Context 修改和普通 Transition
  只允许管理员紧急修复
```

---

## 6. 管理员紧急修复

管理员紧急修复命令仅包含：

```text
MOVE_TO_NODE
TERMINATE_INSTANCE
```

约束：

* 不修改旧 NodeVisit；
* 始终创建新的目标 NodeVisit；
* 目标必须属于当前固定的 Definition Version；
* `MOVE_TO_NODE` 目标必须是非终态节点；
* `TERMINATE_INSTANCE` 目标必须是终态节点；
* 服务端在锁内计算修复前快照 digest；
* 新事实、投影、Event、Receipt、SecurityAudit 在同一事务内提交。

---

## 7. 初始循环外键

实例自引用的循环外键使用：

```sql
DEFERRABLE INITIALLY DEFERRED
```

创建实例前必须预生成以下标识：

```text
instanceId
contextRevisionId
nodeVisitId
```

---

## 8. 导入规则

导入时统一：

```text
workflowStateVersion = 1
eventSequence        = 1
```

Legacy Creator 映射优先级：

1. 优先映射 Legacy Creator；
2. 无法映射时使用当前 Domain Owner，并在导入 Event 中记录 fallback。

---

## 9. Shadow 迁移与 Cutover

Shadow 迁移必须使用持久 Relay 或等价持久机制，不能只依赖进程内同步调用。

Cutover 前必须依次完成：

```text
1. 停止目标 Domain 新写入
2. 排空 Relay
3. 执行全量对账
4. 确认一致后原子切换
```

全量对账至少比较以下字段：

```text
nodeId
assigneePrincipalId
terminal 状态
最后 transitionEffect
Context digest
```
