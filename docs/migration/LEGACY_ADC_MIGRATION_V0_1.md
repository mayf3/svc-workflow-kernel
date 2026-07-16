# Legacy ADC 迁移勘误 v0.1

```text
Status: CURRENT_MIGRATION_BOUNDARY
Version: v0.1
Architecture: SVC_WORKFLOW_ARCHITECTURE_V0_3_1 (ARCHITECTURE_FROZEN)
Last Read-only Check: 2026-07-15
ADC Evidence: develop@343afa49475e6504b61e0b6510bfae372c65027f
SHADOW_NOT_READY
CUTOVER_NOT_READY
```

> 约束：本文件不修改已冻结领域架构，不承诺任何迁移方案。
> 所有迁移方案必须基于架构基线 `SVC_WORKFLOW_ARCHITECTURE_V0_3_1.md`
> 与实施契约 `IMPLEMENTATION_CONTRACT_V0_1.md`。

---

## 1. 调查结论

总体判定：`READY_WITH_BLOCKING_MIGRATION_GAPS`

阻塞缺口（必须在 PR 6B Shadow/Cutover 前由 agent-dev-center 仓库解决）：

### B1. Startup 模板覆盖（HIGH）

`backend/src/lib/workflow-templates.ts` 中 `ensureWorkflowTemplates()` 仍在服务启动时执行并可更新模板 `steps`。需要 ADC 改为仅首次初始化或仅添加不修改。

### B2. 非标准 currentStep 写路径（HIGH）

报告批准自动推进、通用 PATCH 等路径仍可直接更新 `currentStep`，且没有 Relay/Outbox。需要 ADC 将全部写路径收口到标准命令或同一 Relay 边界。

### B3. Domain Owner 非唯一（HIGH）

当前 Legacy schema 仍没有唯一 Domain Owner 约束。svc-workflow 要求一个 Domain 只有一个 enabled Owner；迁移前必须完成规范化。

---

## 2. 身份映射

`principalId` = auth-service `User.id` (UUID)。

| Legacy Identity | 映射方式 | 稳定性 |
|---|---|---|
| `Requirement.assigneeId` (UUID) | 直接映射 | 稳定 |
| `Requirement.requesterId` (UUID) | 直接映射 | 稳定 |
| `User.agentId` (String) | 通过 `User.agentId → User.id` 解析 | 稳定 |

无法映射的 Creator 使用 `DOMAIN_OWNER_FALLBACK`。

Migration Service Principal 应创建为固定 auth-service User。

---

## 3. Workflow Context 边界

开发流程 contextSchema 候选（仅调查结论，不修改冻结架构）：

```json
{
  "type": "object",
  "required": ["title", "description", "domainKey"],
  "properties": {
    "title": { "type": "string", "minLength": 1 },
    "description": { "type": "string", "minLength": 1 },
    "acceptanceCriteria": { "type": "array", "items": { "type": "string" } },
    "domainKey": { "type": "string" },
    "type": { "type": "string", "enum": ["FEATURE", "BUGFIX", "INFRA", "SECURITY"] },
    "repoPath": { "type": "string" },
    "branch": { "type": "string" },
    "gitHash": { "type": "string" }
  }
}
```

不包含 `priority`、`projectId`、`tags`、`dueDate` 等业务字段。

---

## 4. 模板映射

| ADC 概念 | svc-workflow 概念 | 备注 |
|---|---|---|
| `WorkflowTemplate.name` | `WorkflowDefinition.definitionKey` | 机械转换 |
| `steps[].name` | `NodeDefinition.nodeKey` | 机械转换 |
| `steps[].role=requester` | `assigneeRef=WORKFLOW_CREATOR` | 机械转换 |
| `steps[].role=cto/ops/qa` | `assigneeRef=FIXED_PRINCIPAL` 或 `DOMAIN_OWNER` | 需要人工规则 |
| 数组顺序 | `primaryAdvanceTransitionId` 链 | 需要从顺序生成 |
| `rejectTo` | `RETURN` Transition | 需要枚举所有路径 |
| 无对应 | `TERMINATE` Transition | 新增 |
| 无对应 | `contextSchema` | 新增 |
| `requiredReports` | `submissionSchema` | 需要根据 ReportType 定义 |

---

## 5. Submission 迁移分类

| 分类 | 旧数据 | 条件 |
|---|---|---|
| SAFE_TO_IMPORT_AS_SUBMISSION | `RequirementReport` (approved) | 可关联到某步骤 |
| IMPORT_AS_LEGACY_REFERENCE | `RequirementReport` (rejected)、`RequirementAuditLog`、`WorkflowTransition` | 保留为历史参考 |
| KEEP_ONLY_IN_LEGACY | `Requirement`、`ExecutionLease`、`FeedbackEvent`、`TestEnvLock` | 上层业务数据 |
| UNMAPPABLE | 无法确定 NodeVisit 归属的旧报告 | 不得伪造为 committed Submission |

---

## 6. 推荐第一条 Shadow 垂直闭环

**推荐：开发 Requirement 流程**（不推荐 llm-todo）

模板：`hotfix`（3 步）或 `backend-dev`（14 步）
Context：见第 3 节 contextSchema
节点：`draft`(DRAFT) → `dev_self_check`(NORMAL) → `done`(TERMINAL)
负责人：WORKFLOW_CREATOR → FIXED_PRINCIPAL → 无
Transition：ADVANCE, RETURN

---

## 7. Shadow Relay 设计要点

推荐插入点：`casUpdateRequirement()` 成功后、事务提交前。

Relay 最小字段：`id`, `domain_key`, `requirement_id`, `event_type`, `current_step`, `assignee_id`, `state_version`, `relay_payload`, `idempotency_key`, `relay_status`, `created_at`

去重键：`legacy:<domainKey>:<requirementId>:<stateVersion>`

当前无 Outbox 模式可复用。

---

## 8. 数据库部署

- ADC 使用 PostgreSQL 16，已有运行实例
- auth-service 共享 ADC 数据库
- llm-todo 使用独立 SQLite
- svc-workflow 使用同一 PostgreSQL 集群，独立 `svc_workflow` database，`workflow` schema

---

## 9. 历史调查证据

旧版完整只读调查是历史快照，不再放在当前树。需要追溯当时的文件路径、行号和
实体表格时使用：

```bash
git show ba005e2:docs/migration/LEGACY_ADC_READ_ONLY_INVESTIGATION_REPORT.md
```
