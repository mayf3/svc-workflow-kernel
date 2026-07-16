# Legacy ADC Initial Import Contract v0.1

```text
Status: CURRENT_MERGED
Domain Baseline: v0.3.1
LOCAL_IMPORT_READY
SHADOW_NOT_READY
CUTOVER_NOT_READY
```

本合同只定义“已经冻结、映射、正规化的单条 ADC 快照”如何原子写入
`svc-workflow` 初始事实。它不是 ADC 连接器，也不代表 Shadow 或 Cutover 已就绪。

## 1. 命令边界

`ImportLegacyWorkflowInstance` 只能由 enabled `SERVICE` principal 发起。目标 Domain
必须 enabled，且恰好存在一个 enabled `WORKFLOW_MIGRATION` binding；命令 actor 必须是
该 binding 的 principal。目标 Definition Version 必须属于该 Domain 且为 `PUBLISHED`，
目标 Node 必须属于该固定 Version。

服务端固定生成：

```text
idempotency_key    = migration:adc:<lowercase legacy UUID>:v1
external_reference = migration:adc:<lowercase legacy UUID>:v1
```

caller 不能覆盖这两个值。request hash 覆盖 route parameters 与完整 request body，但不
包含 idempotency key。相同 actor/key/request 重放存储的响应；不同 request 冲突；
`PROCESSING` 返回可重试错误。每次重放都重新校验当前 actor、Domain、Definition 和
binding。确定性失败完成 Receipt；基础设施失败回滚 Receipt 与全部事实。
后续合法 revise/transition 不改变本导入命令的存储响应；原请求重放仍返回初始
context、visit、event 与 `1/1` 版本结果。

request hash envelope 的 key 固定使用 `camelCase`。所有 `Option::None`（包括 creator
和 URL）必须序列化为显式 `null`；不得省略字段。canonical JSON 与 SHA-256 golden
固定在 `legacy_import/request_hash_contract.rs`，派生的 idempotency key 与 external
reference 不进入 envelope。

## 2. 快照与摘要

`LegacyAdcImportSnapshotV1` 的精确字段为：

```json
{
  "schemaVersion": "ADC_WORKFLOW_IMPORT_SNAPSHOT_V1",
  "requirementId": "uuid",
  "domainKey": "string",
  "workflowId": "string",
  "workflowSnapshot": {},
  "currentStep": "string",
  "assigneeId": null,
  "requesterId": null,
  "stateVersion": 0,
  "updatedAt": "RFC3339",
  "contextPayload": {}
}
```

摘要算法固定为 `SHA256(JCS(LegacyAdcImportSnapshotV1))`。caller 必须提供 64 位小写
SHA-256；服务端重算并比较后才写入事实。`legacyRecordId` 必须等于
`snapshot.requirementId`，Domain Key 与 Node Key 也必须精确匹配。`workflowId` 与
`workflowSnapshot` 只是不可变摘要证据；上游负责 Definition 映射，本服务不声称能
从二者自动证明映射正确。

`workflowSnapshot` 不得为 null，不得含任意层级的 `roleUserMap`。已知 Legacy
伪状态（包括 `archived`、`abandoned`、`rejected`、`in_progress`）不得作为 Node
导入。`contextPayload` 是 Context 的唯一来源。

## 3. Creator 与 Assignee

caller 提供 `legacyCreatorPrincipalId` 时，它必须与 `snapshot.requesterId` 相同，否则
拒绝；相同且解析为 enabled `HUMAN`/`AGENT` 时，Creator Resolution 为
`LEGACY_CREATOR`。未提供，或相同但该 principal 不可用时，才允许回退到目标 Domain
唯一 enabled `HUMAN`/`AGENT` Owner，Resolution 为 `DOMAIN_OWNER_FALLBACK`。
Migration SERVICE 永远不能成为 Creator。

非 Terminal Node 必须携带正规化 `assigneeId`，并与固定 Definition 的
`WORKFLOW_CREATOR`、`DOMAIN_OWNER` 或 `FIXED_PRINCIPAL` resolver 结果精确相同。
Terminal Node 必须没有 assignee，写入的 Node Visit assignee 为 null。

## 4. 原子初始事实

一次成功命令在一个 PostgreSQL transaction 内创建：

- `WorkflowInstance`，`workflow_state_version=1`；
- `WorkflowContextRevision #1`；
- `NodeVisit #1`；
- `WorkflowEvent #1`；
- 完成的 `WorkflowCommandReceipt`；
- 成功 Security Audit。

不会创建 Submission。Event 的 canonical type 为 `WORKFLOW_INSTANCE_IMPORTED`，字段
矩阵固定为：source visit、submission、transition effect、from/to node 均为 null；
target visit 与 context revision 指向本命令创建的事实；old/new version 为 `0/1`。

Event Data 必须恰好六个 key：

```json
{
  "legacySystem": "adc",
  "legacyRecordId": "lowercase UUID",
  "legacySnapshotDigest": "64 lowercase hex",
  "importedNodeId": "lowercase UUID",
  "importedAt": "YYYY-MM-DDTHH:MM:SSZ",
  "creatorResolution": "LEGACY_CREATOR"
}
```

`creatorResolution` 也可为 `DOMAIN_OWNER_FALLBACK`。`importedAt` 由服务端在 transaction
内生成，精确到整秒。Projection rebuild 对 key 数量、所有值、UUID/digest/timestamp
格式、identity 类型、external reference 和初始事实引用做严格重放校验。

## 5. 边界与非目标

`contextPayload`、`workflowSnapshot` 各不超过 1 MiB，metadata 不超过 64 KiB，
external URL 不超过 2048 bytes；Context 必须通过目标 Version 的 JSON Schema。

本切片不轮询 ADC，不实现 Relay、Outbox、worker、high-water mark、comparator、Shadow、
Cutover 或 reverse projection，不修改 ADC/auth/llm-todo，不新增 migration。ADC 启动时
模板覆盖、绕过 Relay 的写路径与 Owner 不唯一问题仍是 Shadow/Cutover blocker。
