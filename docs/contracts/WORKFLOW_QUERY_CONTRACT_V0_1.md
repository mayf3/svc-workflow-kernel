# Workflow Query Service Contract v0.1

```text
Status: CURRENT
Architecture: svc-workflow v0.3.1
Slice: PR 4
Authority: PostgreSQL workflow projection plus immutable facts
```

本合同冻结 PR 4 的读取边界。查询服务不缓存业务状态、不创建第二套实体，也不从 Event
反推 Context Revision、Node Visit 或 Submission。

## 1. 查询与错误

公开查询固定为：

```text
GetWorkflowInstanceDetail
ListWorkflowTimeline
ListContextRevisions
ListNodeVisits
ListSubmissionHistory
ListAssignedToMe
ListCreatorOwnedDrafts
```

错误集合固定为：

```text
PrincipalNotFound
PrincipalDisabled
WorkflowInstanceNotFoundOrNotVisible
RestrictedHistoryNotVisible
InvalidPagination(detail)
InternalConsistency(detail)
StorageError(detail)
```

不存在实例与不可见实例统一返回 `WorkflowInstanceNotFoundOrNotVisible`。响应不得帮助调用者
探测实例是否存在。

## 2. 一致快照与副作用

每次查询必须：

1. 开启一个 PostgreSQL 事务；
2. 在首次读取前设置 `REPEATABLE READ`；
3. 在同一快照内完成 Principal、授权、投影、事实和分页读取；
4. 不使用 `FOR UPDATE`；
5. 成功时不写 Definition、Runtime、Event、Receipt 或 Audit。

事务不能全局设为 `READ ONLY`，因为拒绝路径必须在同一事务写 SecurityAudit。查询只允许
该拒绝审计副作用。审计写入失败时回滚并返回 `StorageError`，绝不放行读取。

## 3. Principal 与可见性

Principal 不存在时返回 `PrincipalNotFound`，不写审计；Principal 存在但 disabled 时，所有
查询均返回 `PrincipalDisabled`，并写：

```text
action = DISABLED_PRINCIPAL_READ_ATTEMPT
```

Domain disabled 不阻断历史或当前读取。实例可见性按以下优先级判定：

| 等级 | 条件 | 范围 |
|---|---|---|
| `DOMAIN_OWNER_FULL` | actor 是当前 enabled `DOMAIN_OWNER` | full |
| `CURRENT_ASSIGNEE_FULL` | actor 是 current Visit assignee，且 current Node 非 TERMINAL | full |
| `CREATOR_DRAFT_FULL` | actor 是 creator，且 current Node 是 DRAFT | full |
| `HISTORICAL_PARTICIPANT_RESTRICTED` | actor 是 creator、曾任 Visit assignee 或曾创建 Submission | restricted |

其他 actor 收到 masked error，并写：

```text
action       = UNAUTHORIZED_WORKFLOW_READ
resourceType = WORKFLOW_INSTANCE
resourceId   = workflowInstanceId
details      = { queryType, reason: NO_VISIBILITY, domainId }
```

不存在的实例不写审计。Domain Owner 更换后必须立即使用新 binding，旧 Owner 不保留权限。

## 4. Detail 形态

`GetWorkflowInstanceDetail` 返回 tagged `Full` 或 `HistoricalParticipant`。

Full 包含：

```text
instance id / domain / definition version and status / creator
currentContextRevisionId / currentNodeVisitId / workflowStateVersion
externalReference / externalUrl / metadata / createdAt
domainEnabled / isTerminal
完整 current Context（chain、payload、digest、creator、time）
完整 current Visit（Node、assignee、instructions、entered transition、time）
current Node 的全部 outgoing Transitions
```

每条 outgoing Transition 包含 target Node 摘要、effect、submission schema、
`executableForActor` 与稳定的 `blockedReason`：

```text
ACTOR_NOT_CURRENT_ASSIGNEE
CURRENT_NODE_TERMINAL
DEFINITION_VERSION_REVOKED
DEFINITION_VERSION_DRAFT
ADVANCE_NOT_PRIMARY
TARGET_ASSIGNEE_UNAVAILABLE
```

`PUBLISHED` 与 `DEPRECATED` 可执行；`REVOKED` 与防御性 `DRAFT` 可读但阻断。target 的
`WORKFLOW_CREATOR`、`DOMAIN_OWNER` 或 `FIXED_PRINCIPAL` 无法解析为 enabled Principal 时，
返回 `TARGET_ASSIGNEE_UNAVAILABLE`。

`ADVANCE` 只有等于 source Node 的 `primaryAdvanceTransitionId` 时才可执行；额外的非-primary
`ADVANCE` 必须返回 `ADVANCE_NOT_PRIMARY`，与写命令的 Transition 选择规则保持一致。

HistoricalParticipant 只含实例公开摘要和 current Node 公开摘要。它不含 creator、current
pointers、external reference/URL、metadata、Context payload、current assignee、instructions
或 outgoing schema。

## 5. 历史读取

### Timeline

Full 可读取全部 Event 领域字段，但 Event DTO 不复制 Context 或 Submission payload。
restricted 只可读取：

```text
绑定 actor 自己 Submission 的 Event
RETURN Submission 的 relatedSubmissionIds 精确引用 actor 自己 Submission 的反馈 Event
进入 TERMINAL Node 或 TERMINATE 的 outcome Event
```

UUID 必须按 JSON array element 精确匹配，禁止字符串或前缀匹配；跨实例引用不得产生可见性。

### Context revisions

只允许 full。restricted 请求写 `UNAUTHORIZED_WORKFLOW_READ`，details reason 为
`RESTRICTED_SCOPE`，提交审计后返回 `RestrictedHistoryNotVisible`。

### Node visits

Full 返回全部 Visit 和 instructions。restricted 只返回 `assigneePrincipalId = actor` 的 Visit，
并移除 instructions；不派生 open/closed/exited 等非权威状态。

### Submission history

Full 返回全部 Submission、绑定的 Context Revision ID 和 source Node 摘要。restricted 只返回：

```text
actor 自己创建的 Submission
RETURN Submission，且 relatedSubmissionIds 精确引用同实例中 actor 自己的 Submission
```

他人的普通 Submission 和跨实例关联始终不可见。

## 6. Worklists

`ListAssignedToMe` 只匹配 instance 的 current Visit pointer，要求其 assignee 为 actor 且
current Node 非 TERMINAL。旧 Visit 和 TERMINAL Visit 不命中。每项包含 Full detail、当前
instructions/outgoing schema/state，以及最近 50 条 upstream Submissions 和最近 50 条 RETURN
Events；还有更多记录时分别设置 `submissionsTruncated` 或 `returnEventsTruncated`，调用方据此
选择对应的独立 history 查询继续翻页。REVOKED/DRAFT 实例仍列出但 blocked。

`ListCreatorOwnedDrafts` 的 DRAFT 指 current Node `node_type = DRAFT`，不是 Definition Version
状态。它只匹配 `createdByPrincipalId = actor`，包含最新 current Context 和 Full detail：

```text
contextEditable = definition status in {PUBLISHED, DEPRECATED}
combinedExecutable = contextEditable
                     AND actor is current assignee
                     AND at least one outgoing transition executable for actor
```

creator 离开 DRAFT 后不再命中；creator 与 current assignee 不同时仍列出，但
`combinedExecutable = false`。

## 7. Keyset pagination

禁止 offset pagination。所有 page 使用 `items + nextCursor`，均以 `limit + 1` 判断是否还有
下一页。

| 查询 | cursor / 顺序 | default | max |
|---|---|---:|---:|
| Timeline | `afterEventSequence`, ASC | 50 | 100 |
| Context revisions | `afterRevisionNumber`, ASC | 50 | 100 |
| Node visits | `(createdAt,id)`, ASC | 50 | 100 |
| Submission history | `(createdAt,id)`, ASC | 50 | 100 |
| Assigned to me | instance `(createdAt,id)`, DESC | 20 | 20 |
| Creator drafts | instance `(createdAt,id)`, DESC | 20 | 50 |

`limit = 0`、超过 max、负数 sequence/revision cursor 返回 `InvalidPagination`。UUID 是相同
timestamp 下的稳定 tie-breaker。

## 8. 防御性一致性

在返回任何 DTO 前必须验证：

```text
instance domain 与 definition domain 一致
current Context pointer 指向本实例的完整 Context fact
current Visit pointer 指向本实例的完整 Visit fact
Visit Node 属于实例绑定的 Definition Version
event sequence 从 1 连续到 workflowStateVersion
event count/max sequence 与 workflowStateVersion 一致
Event 引用的 Context/Visit/Submission 属于同一实例
Submission 的 Context、source Visit、Node 和 Transition 属于同一实例/版本
全部历史 Visit 的 Node 属于实例 Definition Version
Context revisionNumber 从 1 连续，previousRevisionId 紧邻前一 Revision，current pointer 指向链头
```

破坏任一条件返回 `InternalConsistency`，不得返回部分 DTO。

## 9. 存储边界

PR 4 不新增 Migration、缓存、read-model table 或外部索引。PostgreSQL
`workflow_instances` 投影与 Context Revision、Node Visit、Submission、Workflow Event 事实仍是
唯一权威来源。
