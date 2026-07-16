# svc-workflow 串行受治理工作流内核设计 v0.3.1

```text
Status: ARCHITECTURE_FROZEN
Version: v0.3.1
```

> 文档状态：正式架构冻结
> 实现语言：Rust
> 正式数据库：PostgreSQL
> 推荐仓库名：`svc-workflow`
> 产品边界：只管理工作流定义和运行，不拥有 Todo、Requirement、Article、Campaign 等上层业务对象

---

# 一、产品定义

`svc-workflow` 是一个面向固定 Agent 和人类 Principal 的串行受治理工作流内核。

它负责保证：

```text
正确的负责人
在正确的节点
基于明确版本的工作内容
提交符合模板协议的 JSON

工作流按照不可变模板合法流转

每一次内容修改、阶段提交、正常推进、
跨级返回和异常终止都有不可修改的历史
```

`svc-workflow` 不理解业务内容，也不运行 LLM。

---

# 二、能力定位

## 2.1 v0.3.1 支持

```text
单个 Workflow Instance 同时只有一个当前节点

当前节点只有一个具体负责人

每个流程具有一条确定的串行主干

每个非终态节点具有唯一的正常推进方向

后续节点可以返回任意已配置的前序节点

每个节点通过不可修改的 JSON Submission 交付结果

流程创建者可以在 Draft 节点修改版本化 Workflow Context

Domain Owner 可以查看 Domain 内全部流程事实

Agent 可以按自己的 Principal ID 查询当前任务

Agent 可以查看自己的提交后来如何被通过或返回

模板、Context Revision、Node Visit、
Submission 和 Workflow Event 全部可审计
```

---

## 2.2 v0.3.1 不支持

```text
并行节点
正向条件分支
Claim 或任务领取
普通 Reassign
Handoff
Delegate
Timer
外部 Signal
自动 Retry
SLA 编排
任意脚本 Guard
内置 LLM
业务对象存储
跨 Domain 共享模板
运行中更换模板
运行中转移 Domain
```

---

## 2.3 准入标准

一个流程满足以下条件时，可以接入 `svc-workflow`：

> 单一当前节点、单一责任人、确定性正常下一步、JSON 阶段交付。

需要并行审批、动态条件分支、定时调度或复杂外部编排的业务，由上层产品负责。

---

# 三、系统边界

## 3.1 上层产品拥有业务对象

### llm-todo

负责：

```text
Todo 标题
描述
截止时间
优先级
标签
业务侧与 workflowInstanceId 的关联
```

### 开发平台

负责：

```text
Requirement
项目
业务优先级
Git 仓库
发布记录
业务侧与 workflowInstanceId 的关联
```

### 文章平台

负责：

```text
文章正文
作者
封面
发布渠道
发布时间
业务侧与 workflowInstanceId 的关联
```

`svc-workflow` 不保存这些业务对象，也不判断多个 Workflow Instance 是否对应同一个 Todo、需求或文章。

---

## 3.2 不引入 Subject

内核中不存在：

```text
Subject
subjectType
subjectId
subjectSystem
```

上层产品自己保存：

```text
业务对象 ID
→ workflowInstanceId
```

`svc-workflow` 看到的只是独立 Workflow Instance。

---

## 3.3 Workflow Context 的定位

Workflow Context 不是上层业务对象。

它表示：

> 某一次 Workflow Instance 当前正在处理的版本化工作协议和必要输入。

例如开发流程 Context：

```json
{
  "title": "支持不可变工作流模板版本",
  "description": "模板发布以后不能被服务启动覆盖。",
  "acceptanceCriteria": [
    "发布版本不可修改",
    "旧实例继续使用原版本"
  ]
}
```

它只保存完成当前流程所需要的内容，不应被上层长期当作完整业务数据库使用。

---

## 3.4 数据库所有权

允许 `svc-workflow` 和上层产品使用同一个 PostgreSQL 集群。

推荐 `svc-workflow` 使用独立数据库或独立 Schema：

```text
workflow.*
```

表所有权必须明确：

```text
workflow.*
→ 只允许 svc-workflow 写入

llm_todo.*
→ llm-todo 写入

development.*
→ 开发平台写入

article.*
→ 文章平台写入
```

上层产品即使能连接同一个数据库，也不能直接修改工作流表。

所有工作流写操作必须经过 `svc-workflow` Command API。

---

# 四、核心领域模型

核心实体：

```text
Domain
DomainRoleBinding
Principal

WorkflowDefinition
WorkflowDefinitionVersion
NodeDefinition
TransitionDefinition

WorkflowInstance
WorkflowContextRevision
NodeVisit
Submission

WorkflowEvent
CommandReceipt
CommandAttemptAudit
SecurityAudit
OperationalTelemetry
```

内核中不存在：

```text
Item
Subject
Requirement
Todo
Article
Campaign
Comment
Report
Artifact
Evidence
```

“我的工作项”只是 UI 和查询层文案，实际指当前 Principal 负责的 Node Visit。

---

# 五、Principal 与 Domain

## 5.1 Principal

Principal 表示一个具体身份：

```text
Human Principal
Agent Principal
Service Principal
```

每个 Principal 具有稳定唯一的：

```text
principalId
```

当前主要使用固定 Agent。

---

## 5.2 Domain

Domain 表示：

```text
工作流业务归属
工作流模板管理边界
权限边界
审计查看边界
```

最小字段：

```text
domainId
domainKey
displayName
enabled
createdAt
```

Domain 表不保存：

```text
ownerPrincipalId
```

---

## 5.3 Domain Owner 的唯一事实来源

Domain Owner 只通过 `DomainRoleBinding` 表示：

```text
domainId
principalId
roleKey = DOMAIN_OWNER
enabled
createdAt
disabledAt
```

以下所有功能都查询同一条 Binding：

```text
谁是 Domain Owner
谁可以查看 Domain 全部流程
谁可以执行 Domain 审计
DOMAIN_OWNER 节点应分配给谁
Domain 页面应该显示谁
```

禁止在其他表中再保存第二份 Owner 字段。

---

## 5.4 一个 Domain 只能有一个 Owner

冻结规则：

> 每个启用的 Domain 必须恰好有一个有效 Domain Owner。

数据库保证：

```text
同一 domainId
最多一条 enabled DOMAIN_OWNER Binding
```

应用事务保证：

```text
Domain 启用前必须存在 Owner

不能直接删除唯一 Owner

更换 Owner 必须在一个管理事务中完成：
停用旧 Binding
创建新 Binding
写 SecurityAudit
```

Owner 更换不会修改已经创建的 Node Visit。

已经进入某个节点的负责人保持原快照；以后新进入 `DOMAIN_OWNER` 节点的 Visit 使用新 Owner。

---

## 5.5 Domain 权限

建议通用权限：

```text
CREATE_WORKFLOW_INSTANCE
READ_DOMAIN_WORKFLOWS
AUDIT_DOMAIN
MANAGE_WORKFLOW_DEFINITION
PUBLISH_WORKFLOW_DEFINITION
MANAGE_DOMAIN
```

### Domain Owner

可以查看该 Domain 内全部：

```text
Workflow Instance
Context Revision
Node Visit
Submission
正常推进
返回
终止
Agent 反馈
Schema 失败统计
循环和返工统计
```

### 当前节点负责人

天然获得当前 Workflow Instance 的：

```text
读取权限
上游 Context 和 Submission 读取权限
当前节点 Transition 权限
```

它不需要同时成为 Domain 普通成员。

### 历史参与者

可以读取：

```text
自己参与过的 Workflow Instance
自己创建的 Submission
后续针对自己 Submission 的反馈
```

---

# 六、Workflow Definition

## 6.1 Definition 归属 Domain

每个 Workflow Definition 只属于一个 Domain：

```text
workflowDefinitionId
ownerDomainId
definitionKey
displayName
createdAt
```

v0.3.1 不支持一张 Definition 跨多个 Domain 共享。

需要在另一个 Domain 使用相似流程时，创建新的 Definition。

---

## 6.2 Definition Version 生命周期

```text
DRAFT
PUBLISHED
DEPRECATED
REVOKED
```

### DRAFT

可以修改和验证，不能创建正式实例。

### PUBLISHED

可以创建实例，发布后不可修改。

### DEPRECATED

禁止创建新实例，已有实例可以继续运行。

### REVOKED

模板存在严重安全、法规或结构问题。

禁止创建新实例，已有实例停止普通推进，只允许管理员紧急修复。

---

## 6.3 发布时冻结的内容

```text
definitionDigest
JSON Schema dialect
validatorVersion

contextSchema
完整 NodeDefinition
完整 TransitionDefinition
完整 Submission Schema
完整负责人引用
primaryAdvanceTransitionId
节点顺序
```

统一使用：

```text
JCS JSON Canonicalization
+
SHA-256
```

计算 `definitionDigest`。

发布后的 Version 不能被：

```text
启动脚本
Seed
配置同步
代码升级
数据库初始化
```

覆盖。

---

# 七、Workflow Context

## 7.1 Context Schema

每个 Workflow Definition Version 定义：

```text
contextSchema
```

它规定 Workflow Context JSON 必须具有什么结构。

例如：

```json
{
  "type": "object",
  "required": [
    "title",
    "description"
  ],
  "properties": {
    "title": {
      "type": "string",
      "minLength": 1
    },
    "description": {
      "type": "string",
      "minLength": 1
    },
    "acceptanceCriteria": {
      "type": "array",
      "items": {
        "type": "string"
      }
    }
  }
}
```

内核只检查格式，不判断需求是否合理。

---

## 7.2 WorkflowContextRevision

Context 不允许原地修改。

每次保存都创建新的 Revision：

```text
contextRevisionId
workflowInstanceId
revisionNumber
previousRevisionId

payload
payloadDigest

createdByPrincipalId
createdAt
```

Workflow Instance 保存：

```text
currentContextRevisionId
```

---

## 7.3 Context Revision 单链

每个 Workflow Instance 的 Context Revision 必须形成一条单链：

```text
Revision #1
→ Revision #2
→ Revision #3
```

规则：

```text
Revision #1.previousRevisionId = null

后续 Revision.previousRevisionId
必须等于创建前的 currentContextRevisionId

(workflowInstanceId, revisionNumber)
唯一

previousRevisionId 必须属于同一 Workflow Instance
```

---

## 7.4 Context 修改权限

Context 只能在以下条件同时满足时修改：

```text
当前 Node 类型为 DRAFT

调用者等于
WorkflowInstance.createdByPrincipalId
```

其他情况全部只读：

```text
非 Draft 节点不能修改
Domain Owner 不能修改
审核 Agent 不能修改
开发 Agent 不能修改
其他参与者不能修改
```

典型返工流程：

```text
domain_review
→ RETURN 到 draft
→ 创建新的 Draft Node Visit
→ 原创建者修改 Context
→ 再次正常提交
```

---

## 7.5 Context 修改参与状态版本

Context 修改会改变后续 Agent 的工作输入，因此属于工作流状态命令。

只修改 Context 时：

```text
校验当前节点是 Draft
校验调用者是创建者
校验 expectedWorkflowStateVersion
校验新 Context 符合 contextSchema

创建新 Context Revision
更新 currentContextRevisionId
workflowStateVersion + 1

创建一条 WorkflowEvent
完成 CommandReceipt
```

---

## 7.6 修改 Context 并流转

创建者可以在一个命令中完成：

```text
创建新 Context Revision
创建 Submission
执行 primary ADVANCE Transition
创建目标 Node Visit
更新 Instance Projection
```

冻结规则：

> 一个成功状态命令，`workflowStateVersion` 只增加 1，并且只创建一条 WorkflowEvent。

组合命令 Event：

```text
WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED
```

关系必须为：

```text
newRevision.previousRevisionId
= 命令前 currentContextRevisionId

Submission.contextRevisionId
= newRevision.contextRevisionId
```

---

# 八、Node Definition

最小字段：

```text
nodeId
nodeKey
displayName
orderIndex
nodeType
assigneeRef
instructions
primaryAdvanceTransitionId
```

`nodeType`：

```text
DRAFT
NORMAL
TERMINAL
```

---

## 8.1 Draft Node

每张模板必须有且只有一个 Draft 入口节点。

Draft Node：

```text
必须是主干入口
负责人必须为 WORKFLOW_CREATOR
允许创建者修改 Context
必须指定唯一 primaryAdvanceTransitionId
可以有指向异常终态的终止边
```

---

## 8.2 Normal Node

Normal Node：

```text
具有具体负责人
Context 只读
必须指定唯一 primaryAdvanceTransitionId
可以有零到多条 RETURN 边
可以有零到多条异常 TERMINATE 边
```

---

## 8.3 Terminal Node

Terminal Node：

```text
没有负责人
没有出边
不接受新的普通业务操作
```

示例：

```text
done
abandoned
duplicate
rejected
```

进入 Terminal Node 后：

```text
WorkflowInstance.currentNodeVisitId
```

仍指向对应终态 Visit。

系统不存在第二套：

```text
ACTIVE
COMPLETED
CANCELLED
```

---

# 九、负责人解析

支持三种 `assigneeRef`：

```text
WORKFLOW_CREATOR
DOMAIN_OWNER
FIXED_PRINCIPAL
```

## WORKFLOW_CREATOR

解析为：

```text
WorkflowInstance.createdByPrincipalId
```

## DOMAIN_OWNER

解析为当前唯一有效：

```text
DomainRoleBinding(roleKey = DOMAIN_OWNER)
```

## FIXED_PRINCIPAL

模板直接指定固定 `principalId`。

进入 Node 时，解析结果快照到：

```text
NodeVisit.assigneePrincipalId
```

后续模板或 Domain Owner 改变，不追溯修改旧 Node Visit。

---

# 十、Workflow 图结构

## 10.1 主干定义

每个非终态 Node 必须指定：

```text
primaryAdvanceTransitionId
```

该 Transition 构成唯一正常主干。

例如：

```text
draft
→ domain_review
→ design
→ development
→ review
→ acceptance
→ done
```

主干最后一条：

```text
acceptance → done
```

虽然目标是 Terminal Node，但因为它是 `primaryAdvanceTransitionId`，仍属于正常 ADVANCE，而不是异常 TERMINATE。

---

## 10.2 Transition Effect

执行时根据模板和图结构计算：

### ADVANCE

```text
当前 Node 的 primaryAdvanceTransitionId
```

即使目标是成功 Terminal Node，仍为 ADVANCE。

### RETURN

```text
目标是 orderIndex 更小的非终态 Node
```

### TERMINATE

```text
不是 primaryAdvanceTransitionId
并且目标是 Terminal Node
```

---

## 10.3 统计语义

```text
ADVANCE
→ 正常推进，包括正常完成

RETURN
→ 返工或驳回

TERMINATE
→ 放弃、重复、错误流程等异常结束
```

只有 RETURN 计入：

```text
返工率
驳回率
Agent 失败反馈
```

TERMINATE 不污染 Agent 驳回统计。

---

## 10.4 模板发布校验

发布前必须检查：

```text
恰好一个 Draft 入口节点

每个 node.orderIndex 在版本内唯一

主干 primary Transition 严格向更高 orderIndex 前进

primary 主干无环

每个非终态 Node 恰好一个 primaryAdvanceTransitionId

主干最终到达一个 Terminal Node

RETURN 只能指向更低 orderIndex 的非终态 Node

异常 TERMINATE 只能指向 Terminal Node

Terminal Node 没有出边

所有 Node 从 Draft 可达

所有 assigneeRef 有效

所有 FIXED_PRINCIPAL 存在且启用

所有 Context 和 Submission Schema 合法
```

---

# 十一、Workflow Instance

最小字段：

```text
workflowInstanceId
domainId
definitionVersionId

createdByPrincipalId

currentContextRevisionId
currentNodeVisitId
workflowStateVersion

externalUrl
metadata

createdAt
```

其中：

```text
externalUrl
→ 可选，上层业务页面跳转地址

metadata
→ 可选的轻量展示或关联信息
```

`externalUrl` 和 `metadata` 在 v0.3.1 创建后不可修改。

需要修改的标题、描述、验收标准等内容必须放入版本化 Workflow Context。

---

# 十二、Node Visit

Node Visit 表示某个 Workflow Instance 第几次进入某个 Node。

最小字段：

```text
nodeVisitId
workflowInstanceId
nodeId
visitNumber
assigneePrincipalId
enteredByTransitionId
createdAt
```

Node Visit 创建后不可修改。

不保存权威：

```text
exitedAt
OPEN
CLOSED
ACTIVE
```

退出时间由创建下一 Node Visit 的 Workflow Event 时间推导。

---

## 12.1 唯一当前节点

每个 Workflow Instance 始终只有一个：

```text
currentNodeVisitId
```

它是当前节点的唯一查询投影。

---

## 12.2 Visit 编号

同一 Workflow Instance 多次进入同一 Node 时：

```text
development visit #1
development visit #2
development visit #3
```

数据库保证：

```text
(workflowInstanceId, nodeId, visitNumber)
唯一
```

---

## 12.3 每个 Visit 最多一个 committed Submission

失败请求不创建 Submission。

例如：

```text
Schema 校验失败
版本冲突
非当前负责人
非法 Transition
幂等键冲突
```

这些进入 `CommandAttemptAudit`。

---

# 十三、Submission

## 13.1 唯一阶段交付原语

内核不区分：

```text
Report
Comment
Artifact
Evidence
```

所有正式阶段交付统一为不可修改的 JSON Submission。

最小字段：

```text
submissionId
workflowInstanceId
sourceNodeVisitId
contextRevisionId

authorPrincipalId
transitionId

payload
payloadDigest
schemaVersion

createdAt
```

Submission 创建后不能修改或删除。

---

## 13.2 Submission Schema

每条 Transition 定义自己的 JSON Schema。

开发完成示例：

```json
{
  "type": "object",
  "required": [
    "branchName",
    "selfCheckReport",
    "testSummary",
    "knownRisks"
  ],
  "properties": {
    "branchName": {
      "type": "string",
      "minLength": 1
    },
    "selfCheckReport": {
      "type": "object",
      "required": [
        "summary",
        "changedFiles"
      ]
    },
    "testSummary": {
      "type": "object",
      "required": [
        "result",
        "commands"
      ]
    },
    "knownRisks": {
      "type": "array"
    }
  }
}
```

内核只检查结构。

内容真实性由下一个 Agent 通过自己的 Skill 和工具判断。

---

## 13.3 Resource Reference

大文件、代码、图片或外部系统结果使用通用引用：

```json
{
  "type": "resource-ref",
  "uri": "https://example/resource",
  "digest": "sha256:...",
  "mediaType": "application/json"
}
```

内核只保存引用和摘要，不理解业务含义。

---

## 13.4 ADVANCE Submission

字段要求完全由 Transition Schema 决定。

简单 Todo 可以允许：

```json
{}
```

复杂开发流程可以要求完整自检、分支和测试信息。

---

## 13.5 RETURN Submission

RETURN 必须至少包含：

```text
rootCauseNodeVisitId
relatedSubmissionIds
reasonCode
reason
```

示例：

```json
{
  "rootCauseNodeVisitId": "development-visit-1",
  "relatedSubmissionIds": [
    "submission-123"
  ],
  "reasonCode": "IMPLEMENTATION_INCORRECT",
  "reason": "数据库迁移不支持回滚。",
  "suggestedFix": "补充失败恢复和回滚实现。"
}
```

其中：

```text
transitionId
→ 决定返回哪个 Node

rootCauseNodeVisitId
→ 指明问题最早来自哪轮

relatedSubmissionIds
→ 指明哪些正式提交存在问题
```

RETURN 后创建目标 Node 的全新 Node Visit，不重新打开旧 Visit。

---

## 13.6 TERMINATE Submission

普通业务 TERMINATE 必须有 Submission。

至少要求：

```text
reasonCode
reason
```

适用于：

```text
主动放弃
重复实例
错误流程
不再需要
```

正常主干进入 `done` 属于 ADVANCE，不强制填写异常终止原因。

管理员紧急终止是唯一可以不创建普通业务 Submission 的例外。

---

# 十四、状态命令语义

冻结规则：

> 一个成功状态命令，`workflowStateVersion + 1`，并且恰好创建一条 WorkflowEvent。

包括：

```text
创建实例
只修改 Context
执行 Transition
修改 Context 并 Transition
管理员紧急修复
```

创建实例后：

```text
workflowStateVersion = 1
eventSequence = 1
```

后续成功状态命令：

```text
newWorkflowStateVersion
= oldWorkflowStateVersion + 1

eventSequence
= newWorkflowStateVersion
```

因此状态版本与实例内成功状态事件严格一一对应。

---

# 十五、提交即流转

普通 Transition 在同一个 PostgreSQL 事务中完成：

```text
处理幂等

锁定 Workflow Instance

校验 Domain 和实例可见性

校验当前 Node Visit 负责人

校验 expectedWorkflowStateVersion

校验 transitionId 属于当前 Node

计算 ADVANCE / RETURN / TERMINATE

校验 Context Revision

校验 Submission Schema

创建 Submission

创建目标 Node Visit

更新 currentNodeVisitId

workflowStateVersion + 1

创建一条 WorkflowEvent

完成 CommandReceipt
```

任何一步失败，工作流事实和投影都不能部分提交。

---

# 十六、实例创建

创建请求：

```text
domainId
definitionVersionId
initialContext
externalUrl 可选
metadata 可选
idempotencyKey
```

创建时不要求：

```text
expectedWorkflowStateVersion
```

创建事务：

```text
校验调用方拥有 CREATE_WORKFLOW_INSTANCE

校验 Domain 已启用

校验 Definition 属于该 Domain

校验 Definition Version 为 PUBLISHED

校验 initialContext 符合 contextSchema

创建 Workflow Instance

创建 Context Revision #1

创建 Draft Node Visit

Draft assignee = createdByPrincipalId

设置 currentContextRevisionId

设置 currentNodeVisitId

设置 workflowStateVersion = 1

创建 WORKFLOW_INSTANCE_CREATED Event

完成 CommandReceipt
```

不存在“已创建但未开始”的额外状态。

---

# 十七、Agent 获取任务

Agent 通过认证 Principal 查询：

```http
GET /workflow-instances/assigned-to-me
```

返回至少包括：

```text
Workflow Instance
当前 Context Revision
当前 Node Visit
Node instructions
可执行 Transition
每条 Transition Submission Schema
上游 committed Submission
相关 RETURN 历史
workflowStateVersion
```

Agent 工作方式：

```text
查询自己的任务
→ 阅读 Context 和上游 Submission
→ 使用本地 Skill 和工具执行
→ 提交 JSON
→ svc-workflow 自动流转
```

---

# 十八、Agent 学习与反馈

建议接口：

```http
GET /principals/me/submission-feedback
```

Agent 可以看到：

```text
自己处理过的 Node Visit

自己提交的 Submission

哪些 Submission 正常通过

哪些 Submission 被直接 RETURN

哪些 Submission 在后续阶段被跨级 RETURN

具体原因和建议

自己参与流程的最终结果
```

该视图由已有：

```text
WorkflowContextRevision
NodeVisit
Submission
WorkflowEvent
```

投影生成，不增加新的领域实体。

---

# 十九、Domain Owner 观察

Domain Owner 可以查看 Domain 内全部：

```text
Workflow Instance
Context Revision
Node Visit
Submission
ADVANCE
RETURN
TERMINATE
Agent 被返回情况
跨级根因分布
节点循环次数
reasonCode 分布
Schema 校验失败统计
```

这些数据可以提供给外部分析 Agent。

`svc-workflow` 本身不调用 LLM，也不自动修改模板。

---

# 二十、幂等与并发

## 20.1 数据库与隔离

正式数据库：

```text
PostgreSQL
```

普通命令使用：

```text
READ COMMITTED
+
显式行锁
+
唯一约束
```

锁顺序固定为：

```text
1. CommandReceipt 幂等键
2. WorkflowInstance
3. 必要的 Domain / Definition 只读数据
```

禁止不同命令使用不同锁顺序。

---

## 20.2 命令分类

### 创建命令

要求：

```text
idempotencyKey
```

### 已有实例状态命令

要求：

```text
idempotencyKey
expectedWorkflowStateVersion
```

包括：

```text
Context 修改
Context 修改并 Transition
Transition
管理员紧急修复
```

---

## 20.3 幂等键作用域

```text
principalId + idempotencyKey
```

数据库唯一。

---

## 20.4 requestHash

统一计算：

```text
JCS({
  commandSchemaVersion,
  commandType,
  routeParameters,
  completeRequestBodyWithoutIdempotencyKey
})
→ SHA-256
```

这会覆盖：

```text
domainId
definitionVersionId
instanceId
transitionId
expectedWorkflowStateVersion
initialContext
contextPayload
submissionPayload
externalUrl
metadata
管理员操作目标
管理员操作原因
```

---

## 20.5 CommandReceipt

最小字段：

```text
commandId
principalId
idempotencyKey
commandType
requestHash

receiptStatus
responseStatus
responseBody
responseDigest

createdAt
completedAt
```

`receiptStatus`：

```text
PROCESSING
COMPLETED
```

只允许：

```text
PROCESSING → COMPLETED
```

完成后不可覆盖。

---

## 20.6 并发流程

```text
BEGIN

尝试插入 PROCESSING Receipt

如果唯一键冲突：
读取并锁定已有 Receipt

再次比较 requestHash

已有 COMPLETED 且 Hash 相同：
返回首次响应

已有 Receipt 但 Hash 不同：
返回 409

新命令：
锁定 Workflow Instance
执行权限、版本、Schema 和 Transition 校验
写工作流事实与投影
将 Receipt 更新为 COMPLETED

COMMIT
```

如果两个相同请求同时到达，第二个请求必须等待并返回第一个请求的原始结果。

具体 PostgreSQL 实施应使用：

```sql
INSERT INTO command_receipts (...)
VALUES (...)
ON CONFLICT (principal_id, idempotency_key) DO NOTHING
RETURNING command_id;
```

如果没有插入成功，再执行：

```sql
SELECT *
FROM command_receipts
WHERE principal_id = $1
  AND idempotency_key = $2
FOR UPDATE;
```

相同 `idempotencyKey`、不同 `requestHash` 时：

```text
原 CommandReceipt 永远不修改
写 CommandAttemptAudit
返回 409
```

只有成功插入自己 `PROCESSING` Receipt 的请求，才能将该 Receipt 完成为成功结果或确定性失败结果。

---

## 20.7 失败结果

确定性的命令失败可以完成自己创建的 Receipt，例如：

```text
版本冲突
非当前负责人
Schema 不合法
非法 Transition
```

同时写 `CommandAttemptAudit`。

相同幂等键但不同 requestHash 的冲突请求不能完成或修改原 Receipt，只记录 `CommandAttemptAudit` 并返回 409。

基础设施失败不完成 Receipt，例如：

```text
数据库连接中断
进程崩溃
事务无法提交
```

此时事务回滚，客户端可以安全重试。

---

# 二十一、Submission 与 Context 绑定

## 21.1 Transition-only

普通 Transition 不允许客户端自由选择历史 Context Revision。

服务端锁定 Workflow Instance 后，自动绑定：

```text
Submission.contextRevisionId
=
WorkflowInstance.currentContextRevisionId
```

即使客户端为了显式并发校验而携带 `contextRevisionId`，该值也必须等于锁内读取的 `currentContextRevisionId`，否则拒绝。

---

## 21.2 Context Revision + Transition

组合命令中：

```text
newRevision.previousRevisionId
=
命令开始前的 currentContextRevisionId
```

同时：

```text
Submission.contextRevisionId
=
本事务创建的 newRevisionId
```

---

## 21.3 RETURN 引用完整性

除 JSON Schema 校验外，内核必须验证：

```text
rootCauseNodeVisitId
必须属于当前 Workflow Instance

relatedSubmissionIds
必须全部属于当前 Workflow Instance

所有引用的 Visit 和 Submission
必须在当前命令前已经存在

禁止跨 Workflow Instance 引用

调用者必须有权读取被引用的记录
```

这些属于内核数据完整性检查，不属于业务 Validator。

---

# 二十二、四类不可变权威事实

共同构成工作流权威历史：

```text
WorkflowContextRevision
NodeVisit
Submission
WorkflowEvent
```

Workflow Instance 中：

```text
currentContextRevisionId
currentNodeVisitId
workflowStateVersion
```

属于可重建查询投影。

---

# 二十三、Workflow Event

最小字段：

```text
eventId
workflowInstanceId
eventSequence
eventSchemaVersion

commandId
causationId
correlationId

eventType
transitionEffect

sourceNodeVisitId
targetNodeVisitId

contextRevisionId
submissionId

eventData
eventDataDigest

actorPrincipalId
fromNodeId
toNodeId

oldWorkflowStateVersion
newWorkflowStateVersion

createdAt
```

不保存含义模糊的通用 `payloadDigest`。

Context 和 Submission 分别保存自己的 `payloadDigest`。

---

## 23.1 Event 字段冻结矩阵

| eventType                                           | sourceNodeVisitId | targetNodeVisitId | contextRevisionId | submissionId |
| --------------------------------------------------- | ----------------- | ----------------- | ----------------- | ------------ |
| `WORKFLOW_INSTANCE_CREATED`                         | `null`            | 初始 Draft Visit    | Revision #1       | `null`       |
| `WORKFLOW_CONTEXT_REVISED`                          | 当前 Visit          | 同一当前 Visit        | 新 Revision        | `null`       |
| `WORKFLOW_TRANSITION_COMMITTED`                     | 旧 Visit           | 新 Visit           | 命令时当前 Revision    | 新 Submission |
| `WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED` | 旧 Draft Visit     | 新 Visit           | 新 Revision        | 新 Submission |
| `WORKFLOW_INSTANCE_IMPORTED`                        | `null`            | 导入 Visit          | 导入 Revision       | `null`       |
| `ADMIN_EMERGENCY_OVERRIDE_COMMITTED`                | 旧 Visit           | 新 Visit           | 命令完成后的当前 Revision | `null`       |

统一规则：

```text
targetNodeVisitId
=
命令完成后的 WorkflowInstance.currentNodeVisitId

contextRevisionId
=
命令完成后的 WorkflowInstance.currentContextRevisionId
```

---

## 23.2 Event 与 Command 约束

```text
WorkflowEvent.commandId
→ CommandReceipt.commandId
```

每一个成功状态 Command：

```text
最多且应当恰好对应一个 WorkflowEvent
```

同时：

```text
newWorkflowStateVersion
=
oldWorkflowStateVersion + 1

eventSequence
=
newWorkflowStateVersion
```

Event 引用的：

```text
sourceNodeVisitId
targetNodeVisitId
contextRevisionId
submissionId
```

都必须属于同一个 Workflow Instance。

---

## 23.3 核心 Event 类型

```text
WORKFLOW_INSTANCE_CREATED

WORKFLOW_CONTEXT_REVISED

WORKFLOW_TRANSITION_COMMITTED

WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED

WORKFLOW_INSTANCE_IMPORTED

ADMIN_EMERGENCY_OVERRIDE_COMMITTED
```

不同 Event 的额外内容放入：

```text
eventData
```

其结构由：

```text
eventType + eventSchemaVersion
```

决定。

---

## 23.4 不提供全局事件游标

v0.3.1 不保存：

```text
globalEventCursor
```

只保证每个 Workflow Instance 内：

```text
eventSequence
```

严格递增。

未来真正需要跨系统事件流时，再统一设计：

```text
Transactional Outbox
提交顺序
高水位
至少一次投递
消费者幂等
```

不提前提供可能漏事件的伪全局游标。

---

# 二十四、Definition Version 状态门禁

所有普通状态命令必须在事务内读取并校验当前实例固定的 Definition Version 状态。

## PUBLISHED

```text
允许创建新实例
允许已有实例修改 Context
允许已有实例执行普通 Transition
```

## DEPRECATED

```text
禁止创建新实例
允许已有实例修改 Context
允许已有实例执行普通 Transition
```

## REVOKED

```text
禁止创建新实例
禁止已有实例修改 Context
禁止已有实例执行普通 Transition

只允许：
REBUILD_PROJECTION
ADMIN_EMERGENCY_OVERRIDE
```

Definition Version 状态必须在 Workflow Instance 锁定后的事务内稳定读取，避免检查和提交之间状态发生变化。

---

# 二十五、状态投影重建

不通过 Event 重新创建 Node Visit、Context Revision 或 Submission。

正确过程是：

```text
锁定 Workflow Instance

读取并校验：
WorkflowContextRevision
NodeVisit
Submission
WorkflowEvent

验证事件序列和引用关系

重新计算：
currentContextRevisionId
currentNodeVisitId
workflowStateVersion

更新 WorkflowInstance 投影
```

---

# 二十六、审计分层

## 26.1 WorkflowEvent

成功发生的工作流领域事实。

用于：

```text
Timeline
投影重建
Agent 学习
Domain 分析
```

## 26.2 CommandAttemptAudit

已认证 Principal 发起但失败的命令：

```text
Schema 失败
版本冲突
非当前负责人
非法 Transition
幂等内容不一致
```

## 26.3 SecurityAudit

```text
未认证访问
越权读取
越权创建
越权提交
管理员紧急修复
Domain Owner 变更
```

## 26.4 OperationalTelemetry

```text
请求延迟
数据库错误
事务重试
错误率
资源消耗
```

运行遥测不属于工作流业务历史。

---

# 二十七、管理员紧急修复

不使用普通 Reassign。

不把任务日常从 Agent A 转交给 Agent B。

只保留两个仅用于系统异常的管理员能力。

---

## 27.1 REBUILD_PROJECTION

用途：

```text
currentNodeVisitId 损坏
currentContextRevisionId 损坏
workflowStateVersion 不一致
```

规则：

```text
执行前锁定 Workflow Instance

根据四类不可变事实重新计算投影

不改变业务事实

不增加 workflowStateVersion

不创建普通 WorkflowEvent

写 SecurityAudit
```

---

## 27.2 ADMIN_EMERGENCY_OVERRIDE

中文含义：

> 管理员紧急修复。

只允许两个操作：

```text
MOVE_TO_NODE
TERMINATE_INSTANCE
```

### MOVE_TO_NODE

```text
不修改旧 Node Visit

目标 Node 必须属于实例固定的 Definition Version

目标 Node 必须是非终态节点

按照目标 Node 正常解析负责人

解析出的 Principal 必须存在且启用

创建目标 Node 的新 Node Visit

更新 currentNodeVisitId

workflowStateVersion + 1

创建 ADMIN_EMERGENCY_OVERRIDE_COMMITTED Event

完成 CommandReceipt

写 SecurityAudit
```

### TERMINATE_INSTANCE

```text
目标 Node 必须属于实例固定的 Definition Version

目标 Node 必须是 Terminal Node

创建目标 Terminal Node 的新 Node Visit

更新 currentNodeVisitId

workflowStateVersion + 1

创建 ADMIN_EMERGENCY_OVERRIDE_COMMITTED Event

完成 CommandReceipt

写 SecurityAudit
```

管理员紧急终止是唯一不要求普通业务 Submission 的终止方式。

请求必须包含：

```text
idempotencyKey
expectedWorkflowStateVersion
operation
targetNodeId
reason
expectedBeforeSnapshotDigest 可选
相关工单或资源引用
```

修复前实际快照 digest 必须由服务端在锁住 Workflow Instance 后计算。

如果客户端提供 `expectedBeforeSnapshotDigest`：

```text
必须与服务端实际计算值一致
否则拒绝执行
```

新 Node Visit、Instance Projection、WorkflowEvent、CommandReceipt 和 SecurityAudit 必须在同一事务提交。

v0.3.1 不提供：

```text
REASSIGN_CURRENT_VISIT
普通 Reassign
Handoff
Delegate
```

固定 Agent 或 Workflow Creator 失效时，只能：

```text
移动到其他可执行节点
或
终止当前实例后创建新实例
```

不能在原 Node Visit 上更换负责人。

---

# 二十八、数据库硬约束

必须保证：

```text
Workflow Definition：
每个 Definition 只属于一个 Domain

Workflow Definition Version：
PUBLISHED / DEPRECATED / REVOKED 后业务字段不可修改

Domain Owner：
同一 Domain 最多一个 enabled DOMAIN_OWNER Binding

Context Revision：
(workflowInstanceId, revisionNumber) 唯一
previousRevisionId 必须属于同一 Instance
创建后不可修改或删除

Node Visit：
(workflowInstanceId, nodeId, visitNumber) 唯一
创建后不可修改或删除

Submission：
每个 sourceNodeVisitId 最多一个 committed Submission
Submission、Node Visit、Context Revision 必须属于同一 Instance
创建后不可修改或删除

Workflow Event：
(workflowInstanceId, eventSequence) 唯一
commandId 关联 CommandReceipt.commandId
一个成功状态 Command 最多一个 WorkflowEvent
sourceNodeVisitId 和 targetNodeVisitId 必须属于同一 Instance
contextRevisionId 和 submissionId 必须属于同一 Instance
创建后不可修改或删除

Command Receipt：
(principalId, idempotencyKey) 唯一
COMPLETED 后不可修改或删除

Workflow Instance：
currentNodeVisitId 必须属于本 Instance
currentContextRevisionId 必须属于本 Instance
domainId 创建后不可修改
definitionVersionId 创建后不可修改
createdByPrincipalId 创建后不可修改
```

“启用 Domain 恰好一个 Owner”由：

```text
数据库部分唯一索引
+
Domain 管理事务校验
```

共同保证。

---

## 28.1 初始循环外键

Workflow Instance 同时引用初始 Context Revision 和初始 Node Visit，而二者又引用 Workflow Instance。

创建实例时必须预生成：

```text
workflowInstanceId
contextRevisionId
nodeVisitId
```

以下复合外键使用：

```text
DEFERRABLE INITIALLY DEFERRED
```

包括：

```text
WorkflowInstance
(currentContextRevisionId, workflowInstanceId)
→ WorkflowContextRevision
(contextRevisionId, workflowInstanceId)

WorkflowInstance
(currentNodeVisitId, workflowInstanceId)
→ NodeVisit
(nodeVisitId, workflowInstanceId)
```

相关字段保持：

```text
NOT NULL
```

PostgreSQL 在事务提交时统一验证循环引用完整性。

---

# 二十九、大小与安全限制

v0.3.1 服务硬限制：

```text
Context payload：
最大 1 MiB

Submission payload：
最大 1 MiB

metadata：
最大 64 KiB

CommandReceipt responseBody：
最大 1 MiB

eventData：
最大 256 KiB
```

大内容必须使用 `resource-ref`。

安全规则：

```text
不在 Event、Audit 和日志中保存 Token、密码和密钥

日志默认只记录 digest、大小和类型

Resource URI 使用协议和域名白名单

默认禁止 file://

默认禁止未授权本地文件路径

敏感字段支持配置化脱敏

Context 和 Submission 制定数据保留策略
```

---

# 三十、API 边界

## Definition

```http
POST /workflow-definitions
POST /workflow-definitions/:id/versions
POST /workflow-definition-versions/:id/validate
POST /workflow-definition-versions/:id/publish
POST /workflow-definition-versions/:id/deprecate
POST /workflow-definition-versions/:id/revoke
```

## Instance

```http
POST /workflow-instances

GET /workflow-instances/:id
GET /workflow-instances/:id/timeline
GET /workflow-instances/:id/node-visits
GET /workflow-instances/:id/context-revisions
GET /workflow-instances/:id/submissions
```

## Context

```http
POST /workflow-instances/:id/context-revisions
```

## Transition

```http
POST /workflow-instances/:id/transitions
```

支持：

```text
Transition-only
Context Revision + Transition
```

## Agent

```http
GET /workflow-instances/assigned-to-me
GET /principals/me/submission-feedback
```

## Domain

```http
GET /domains/:domainId/workflow-instances
GET /domains/:domainId/audit
```

## Admin

```http
POST /admin/workflow-instances/:id/rebuild-projection
POST /admin/workflow-instances/:id/emergency-override
```

---

# 三十一、Rust 服务设计

## 31.1 仓库

正式推荐：

```text
svc-workflow
```

推荐路径：

```text
/Users/yanfenma/workspace/project/svc-workflow
```

---

## 31.2 模块结构

第一版保持单 crate：

```text
src/
  domain/
    domain/
    definition/
    context/
    instance/
    node_visit/
    submission/
    event/
    permission/

  application/
    commands/
    queries/

  store/
    postgres/
    repositories/
    transaction/

  api/
    routes/
    dto/
    middleware/

  admin/
```

边界稳定后再考虑拆成：

```text
workflow-domain
workflow-application
workflow-store
workflow-api
```

v0.3.1 不提前拆多 crate。

---

## 31.3 独占写入边界

`svc-workflow` 独占负责：

```text
模板版本
图结构验证
Context Schema
Instance
Context Revision
Node Visit
Submission
Transition
幂等
CAS
Workflow Event
Domain 工作流权限
Agent 任务查询
管理员紧急修复
```

---

## 31.4 与 Agent Core 的关系

`svc-workflow` 不并入 Agent Core。

```text
Agent Core：
Agent、Session、Run、Capability、
Approval、Registry、Receipt

svc-workflow：
Definition、Instance、Context、
Node Visit、Submission、Transition
```

未来 Agent Core 可以通过 External Harness 调用 `svc-workflow` API。

双方没有代码依赖。

---

# 三十二、迁移方案

## Phase 0：新服务最小闭环

首先实现：

```text
创建模板
发布模板
创建 Instance
创建 Context Revision
查询 assigned-to-me
ADVANCE
RETURN
再次修改 Context
再次 ADVANCE
正常进入 done
异常 TERMINATE
查询完整 Timeline
幂等并发重试
```

---

## Phase 1：不可变存储骨架

实现：

```text
WorkflowDefinitionVersion
WorkflowInstance
WorkflowContextRevision
NodeVisit
Submission
WorkflowEvent
CommandReceipt
```

优先验证：

```text
数据库约束
事务原子性
幂等并发
事件序列
```

---

## Phase 2：导入旧模板

将旧 workflowSnapshot 转成不可变 Definition Version。

冻结：

```text
Context Schema
Node orderIndex
primaryAdvanceTransitionId
RETURN Transition
TERMINATE Transition
负责人
Submission Schema
```

---

## Phase 3：导入旧数据

旧 Requirement、Todo 等继续由上层系统拥有。

导入使用固定：

```text
Migration Service Principal
```

确定性幂等键：

```text
migration:<legacy-system>:<legacy-record-id>:v1
```

每个导入实例固定：

```text
workflowStateVersion = 1
eventSequence = 1
```

生成：

```text
WorkflowInstance
Context Revision
当前 Node Visit
WORKFLOW_INSTANCE_IMPORTED Event
CommandReceipt
```

导入 Event 的 `eventData` 记录：

```text
legacySystem
legacyRecordId
legacySnapshotDigest
importedNodeId
importedAt
creatorResolution
```

Migration Service Principal 只是导入命令的 Actor，不能自动成为：

```text
WorkflowInstance.createdByPrincipalId
```

创建者解析规则：

```text
优先映射 Legacy Creator

无法映射时：
createdByPrincipalId = 当前 Domain Owner

并记录：
creatorResolution = DOMAIN_OWNER_FALLBACK
```

无法确定所属历史 Visit 的旧报告：

```text
作为 Legacy 数据保留
不转换为 committed Submission
不满足新模板 Schema
```

---

## Phase 4：Shadow

```text
Legacy 仍是权威

旧系统成功状态命令与 Legacy 状态修改
必须通过持久 Relay 或等价持久机制
同步到 svc-workflow

svc-workflow 维护影子状态
```

不能只在 Legacy 请求成功后进行一次进程内 HTTP 调用，因为进程崩溃可能丢失影子命令。

推荐：

```text
Legacy 同一事务：
写入旧状态
+
写入 legacy_workflow_relay
```

Relay Worker 负责：

```text
读取未完成 Relay
调用 svc-workflow
失败后保留并重试
成功后记录完成状态
```

---

## Phase 5：Cutover Barrier

切换某个 Domain 前必须：

```text
停止该 Domain 新写入

记录 Relay 高水位

排空该 Domain Relay 到高水位

执行全量对账

确认一致

原子切换命令路由
```

对账至少比较：

```text
current nodeId

assigneePrincipalId

是否处于 Terminal Node

最后一次 transitionEffect

current Context payloadDigest
```

不能只比较 Legacy `currentStep` 与新 Node ID。

---

## Phase 6：Cutover

按 Domain 原子切换命令路由：

```text
Legacy
→ svc-workflow
```

切换后：

```text
svc-workflow 成为该 Domain 的权威状态源
```

---

## Phase 7：Rollback Window

有限时间内：

```text
svc-workflow 将 Legacy 能表达的字段
反向投影回旧表
```

用于必要时回滚。

---

## Phase 8：Final

稳定后：

```text
停止 Legacy 工作流写入

停止反向投影

旧 currentStep 等字段变为只读或删除

所有工作流写入只经过 svc-workflow
```

---

# 三十三、冻结验收标准

1. 内核不包含 Subject、Todo、Requirement、Article 等业务实体。

2. 上层产品自行保存业务对象与 Workflow Instance 的关联。

3. 每个 Workflow Definition 只属于一个 Domain。

4. 每个启用 Domain 恰好一个有效 Domain Owner。

5. Domain Owner 的唯一来源是 `DomainRoleBinding`。

6. 每个模板恰好一个 Draft 入口节点。

7. Draft 负责人为 Workflow Creator。

8. Context 只能在 Draft 由 Workflow Creator 修改。

9. Context 每次修改创建不可变 Revision。

10. Context Revision 形成同实例单链。

11. Context 修改使 `workflowStateVersion + 1`。

12. 一次成功状态命令只增加一个状态版本。

13. 一次成功状态命令恰好创建一条 WorkflowEvent。

14. `eventSequence = newWorkflowStateVersion`。

15. 每个 Workflow Instance 只有一个 `currentNodeVisitId`。

16. Node Visit 不设置第二套活动状态。

17. Node Visit 创建后不可修改。

18. 每个 Node Visit 最多一个 committed Submission。

19. 每个非终态 Node 指定唯一 `primaryAdvanceTransitionId`。

20. 正常主干进入 done 仍属于 ADVANCE。

21. RETURN 只能指向更早的非终态 Node。

22. 异常 TERMINATE 只能指向 Terminal Node。

23. Terminal Node 没有负责人和出边。

24. 所有阶段交付统一为不可修改 JSON Submission。

25. 每次 Submission 由服务端绑定明确 Context Revision。

26. Transition-only 绑定锁内当前 Context Revision。

27. Context + Transition 绑定本事务创建的新 Revision。

28. 内核只验证 JSON Schema，不判断内容真伪。

29. 内容真实性由下游 Agent 的 Skill 和工具判断。

30. RETURN 必须关联同实例根因 Visit、相关 Submission 和明确原因。

31. 普通 TERMINATE 必须提交原因 Submission。

32. 管理员紧急终止可以不创建普通业务 Submission。

33. Context Revision、Node Visit、Submission、WorkflowEvent 是四类不可变事实。

34. Instance 当前字段是可重建投影。

35. 创建命令只要求 idempotencyKey。

36. 现有实例状态命令要求 idempotencyKey 和 expectedVersion。

37. requestHash 使用完整命令信封的 JCS + SHA-256。

38. CommandReceipt 使用 PostgreSQL `ON CONFLICT DO NOTHING RETURNING` 语义。

39. 相同幂等键、不同 requestHash 不得修改原 Receipt。

40. 相同幂等请求并发执行返回同一首次结果。

41. PostgreSQL 是唯一正式数据库。

42. 幂等锁顺序和 Instance 行锁顺序固定。

43. 工作流事实、投影、Event 和 Receipt 原子提交。

44. 每个实例 eventSequence 严格递增。

45. Event 字段填写符合冻结矩阵。

46. Event 的所有实体引用属于同一个 Workflow Instance。

47. REVOKED Definition Version 禁止普通 Context 修改和 Transition。

48. v0 不提供伪全局事件游标。

49. Agent 可以查询当前分配任务。

50. Agent 可以查看自己的提交后来为何通过或返回。

51. Domain Owner 可以查看 Domain 内全部工作流事实。

52. 管理员恢复只提供投影重建和紧急节点修复。

53. 管理员紧急修复不修改旧 Node Visit。

54. 不提供普通 Reassign。

55. 初始循环外键使用 deferred composite constraints。

56. `workflow.*` 表只能由 `svc-workflow` 写入。

57. llm-todo、开发平台和文章平台保持独立。

58. Shadow 同步使用持久 Relay 或等价机制。

59. Cutover 前执行停止写入、排空 Relay 和全量对账。

60. `svc-workflow` 不依赖 Agent Core。

61. `svc-workflow` 内部不运行 LLM。

---

# 三十四、最终定义

`svc-workflow v0.3.1` 是：

> 一个由 Rust 和 PostgreSQL 实现、与上层业务对象解耦的串行受治理工作流内核。它以不可变 Workflow Definition Version 定义流程，以版本化 Workflow Context 表示当前工作协议，以不可变 Submission 表示阶段交付，以单一当前 Node Visit 表示运行位置，并支持正常推进、跨级返回、异常终止、严格幂等、完整审计和 Agent 反馈学习。

它只保证：

```text
正确的负责人
在正确的节点
基于明确的 Context Revision
提交符合模板 Schema 的 JSON

工作流按照不可变模板合法流转

每一次修改、提交、推进、返回和终止
都有明确且不可修改的历史
```
