# Workflow Transition — Implementation Contract v0.1

```text
Status: IMPLEMENTATION_CONTRACT
Version: v0.1
Architecture: SVC_WORKFLOW_ARCHITECTURE_V0_3_1 (ARCHITECTURE_FROZEN)
PR: 3C / 3D
```

## 1. Command Input

```rust
pub struct ExecuteWorkflowTransitionCommand {
    pub principal_id: PrincipalId,
    pub idempotency_key: String,
    pub command_schema_version: String,
    pub workflow_instance_id: WorkflowInstanceId,
    pub expected_workflow_state_version: i32,
    pub transition_definition_id: TransitionId,
    pub submission_payload: Option<serde_json::Value>,
}
```

The following values are never accepted from the client — they are server-generated or resolved:
`target_node_id`, `target_node_visit_id`, `visit_number`, `resolved_assignee_principal_id`, `submission_id`, `command_id`, `event_id`, `transition_effect`, `source_node_visit_id`, `workflow_state_version_after`, `event_sequence`, `context_revision_id`.

## 2. Authorization

The sole authorized caller for a normal transition is:
```text
command.principal_id == current_node_visit.assignee_principal_id
```

Additionally, the principal must exist and be `enabled = true`.

The following identities do NOT grant transition authority on their own:
- Workflow Creator (unless also the current assignee)
- Domain Owner (unless also the current assignee)
- Domain Member
- Target node assignee

Stable errors:
- `PrincipalNotAssignee` (403)
- `PrincipalDisabled` (403)

This explicitly avoids the semantically incorrect `PrincipalNotFound` pattern from PR 3B.

## 3. Lock Order

Fixed lock order (same as PR 3A and PR 3B):

```
1. CommandReceipt (INSERT ON CONFLICT / SELECT FOR UPDATE)
2. WorkflowInstance (SELECT ... FOR UPDATE)
3. DefinitionVersion (SELECT ... FOR UPDATE)
```

## 4. Transition Selection

The client sends `transition_definition_id` (UUID primary key). The server reads the transition definition and validates:

1. `transition.definition_version_id == instance.definition_version_id`
2. `transition.source_node_id == current_visit.node_id`

### ADVANCE

```text
- transition.effect = ADVANCE
- transition.id == source_node.primary_advance_transition_id
- source node non-TERMINAL
```

### RETURN

```text
- transition.effect = RETURN
- transition.id != source_node.primary_advance_transition_id
- target node non-TERMINAL
- target.order_index < source.order_index
```

### TERMINATE

```text
- transition.effect = TERMINATE
- transition.id != source_node.primary_advance_transition_id
- target.node_type = TERMINAL
```

## 5. Current Visit & Source Node

After locking the instance:
1. Read `instance.current_node_visit_id` → `WorkflowNodeVisit` → `WorkflowNodeDefinition`
2. Verify current visit exists and belongs to this instance
3. Verify source node belongs to the instance's definition version
4. Verify source node is not TERMINAL
5. Verify caller equals current visit's assignee
6. Old NodeVisit is NEVER modified (no UPDATE, no DELETE, no `exited_at`)

## 6. Submission Semantics

### Optional vs Required

| Transition Schema | payload=None | payload=Some(value) |
|---|---|---|
| NULL (no schema) | No submission created, succeeds | Submission created (no schema check) |
| NOT NULL (has schema) | `SubmissionRequired` error | Schema-validated submission |

### Schema Validation

- Uses `workflow_transition_definitions.submission_schema`
- Allows local `#/$defs/...` references
- Rejects network/external/file `$ref`
- Returns `SubmissionValidationFailed` (422)

### Size Limit

- Serialized payload ≤ 1 MiB (service-layer check)
- Database `pg_column_size(jsonb)` as defense in depth

### One Submission Per Visit

Database `UNIQUE (source_node_visit_id)` ensures at most one committed Submission per Visit.

## 7. RETURN Reference Validation

RETURN submissions must include and validate:

```json
{
  "rootCauseNodeVisitId": "uuid",
  "relatedSubmissionIds": ["uuid", ...],
  "reasonCode": "string",
  "reason": "string"
}
```

Validation:
- `rootCauseNodeVisitId` must exist and belong to the current instance
- `relatedSubmissionIds` must all exist and belong to the current instance
- `reasonCode` and `reason` are required
- Cross-instance references are rejected with `InvalidReturnReferences` (422)

## 8. Target Assignee Resolution

Reuses the PR 3A `resolve_assignee` pattern based on target node's `assignee_ref_type`:

| Type | Resolution |
|---|---|
| `WORKFLOW_CREATOR` | `instance.created_by_principal_id` |
| `DOMAIN_OWNER` | Current enabled DOMAIN_OWNER binding |
| `FIXED_PRINCIPAL` | Target node's `fixed_principal_id` |

Resolution result is snapshotted into the new NodeVisit. Future domain owner changes do NOT retroactively modify existing visits.

## 9. Target Visit Number

Calculated within the transaction while holding the instance row lock:

```sql
SELECT COALESCE(MAX(visit_number), 0) + 1
FROM workflow_node_visits
WHERE workflow_instance_id = $1 AND node_id = $2
```

First visit to a node → `visit_number = 1`. RETURN to a previously visited node → `visit_number = previous_max + 1`.

## 10. Instance Projection Update

```sql
UPDATE workflow_instances
SET current_node_visit_id = $new_visit_id,
    workflow_state_version = workflow_state_version + 1
WHERE workflow_instance_id = $id
  AND workflow_state_version = $old_version
  AND current_node_visit_id = $source_visit_id
```

`current_context_revision_id` is NEVER modified by a transition command.

## 11. Event Field Matrix

| Field | Value |
|---|---|
| `event_type` | `WORKFLOW_TRANSITION_COMMITTED` |
| `transition_effect` | `ADVANCE` / `RETURN` / `TERMINATE` |
| `source_node_visit_id` | Old (pre-transition) visit |
| `target_node_visit_id` | New (post-transition) visit |
| `context_revision_id` | Current (unchanged) context revision |
| `submission_id` | New submission ID, or NULL |
| `old_workflow_state_version` | N |
| `new_workflow_state_version` | N + 1 |
| `event_sequence` | N + 1 |
| `actor_principal_id` | Executing principal |
| `command_id` | Receipt command_id |
| `from_node_id` | Source node ID |
| `to_node_id` | Target node ID |

### Event Data

```json
{
  "transitionDefinitionId": "uuid",
  "transitionKey": "string",
  "transitionEffect": "ADVANCE|RETURN|TERMINATE",
  "sourceNodeId": "uuid",
  "targetNodeId": "uuid",
  "sourceNodeVisitId": "uuid",
  "targetNodeVisitId": "uuid",
  "contextRevisionId": "uuid",
  "submissionPayloadDigest": "sha256-hex | null"
}
```

`event_data_digest` = JCS(event_data) → SHA-256.

## 12. Definition Version Lifecycle

| Status | Transition Allowed |
|---|---|
| `PUBLISHED` | ✅ Yes |
| `DEPRECATED` | ✅ Yes (existing instances continue) |
| `REVOKED` | ❌ No (admin emergency override only) |
| `DRAFT` | ❌ No (internal consistency failure) |

## 13. requestHash

Canonical structure (snake_case, no rename):

```json
JCS({
  "command_schema_version": "v1",
  "command_type": "EXECUTE_WORKFLOW_TRANSITION",
  "route_parameters": {},
  "request_body": {
    "principal_id": "<uuid>",
    "workflow_instance_id": "<uuid>",
    "expected_workflow_state_version": 2,
    "transition_definition_id": "<uuid>",
    "submission_payload": null
  }
}) → SHA-256
```

Rules:
- `idempotency_key` is excluded from hash
- `submission_payload = None` serializes as JSON `null`
- JCS sorts all object keys alphabetically

## 14. Success Response

```json
{
  "workflowInstanceId": "uuid",
  "workflowStateVersion": 3,
  "currentContextRevisionId": "uuid",
  "sourceNodeVisitId": "uuid",
  "currentNodeVisitId": "uuid",
  "submissionId": "uuid | null",
  "eventSequence": 3
}
```

`response_digest` = JCS(response) → SHA-256.

## 15. Deterministic vs Infrastructure Failure

### Deterministic (COMPLETED receipt, no runtime facts)

| Condition | Status | Error Code |
|---|---|---|
| Instance not found | 404 | `instance_not_found` |
| Principal disabled | 403 | `principal_disabled` |
| Current visit not found | 404 | `current_visit_not_found` |
| Not current assignee | 403 | `principal_not_assignee` |
| Source node TERMINAL | 409 | `source_node_terminal` |
| Definition version REVOKED | 409 | `definition_version_revoked` |
| Version state conflict | 409 | `workflow_state_version_conflict` |
| Transition not applicable | 409 | `transition_not_applicable` |
| Submission required | 422 | `submission_required` |
| Submission validation failed | 422 | `submission_validation_failed` |
| Size limit exceeded | 413 | `size_limit_exceeded` |
| Invalid RETURN references | 422 | `invalid_return_references` |
| Assignee resolution failed | 422 | `assignee_resolution_failed` |

### Infrastructure (transaction rolls back)

- Submission INSERT failure
- NodeVisit INSERT failure
- Instance UPDATE failure
- Event INSERT failure
- Receipt Completion failure
- Database connection / SQL errors

## 16. Idempotency

Scope: `(principal_id, idempotency_key)`.

Pattern (same as PR 3A/3B):
1. `INSERT ... ON CONFLICT DO NOTHING RETURNING` with `COMMAND_TYPE_EXECUTE_TRANSITION`
2. If row returned → this request owns the transaction
3. If no row → `SELECT ... FOR UPDATE` on existing receipt
   - Same hash + COMPLETED → replay stored response
   - Different hash → `IdempotencyConflict` + `CommandAttemptAudit`
   - PROCESSING → `CommandStillProcessing`

## 17. Concurrency Guarantees

1. **Same key/hash**: Two concurrent calls → one succeeds, the other replays the same result
2. **Different key, same expectedVersion**: One succeeds, one gets `WorkflowStateVersionConflict`
3. **Same key, different hash**: One succeeds, one gets `IdempotencyConflict`
4. **Context Revision + Transition**: Instance `FOR UPDATE` row lock serializes both. One succeeds, the other gets stale version conflict.

## 18. PR 3D — ReviseContextAndTransition

PR 3D 是单独的原子命令，不是客户端依次调用 revise-only 和 transition-only。

```rust
pub struct ReviseContextAndTransitionCommand {
    pub principal_id: PrincipalId,
    pub idempotency_key: String,
    pub command_schema_version: String,
    pub workflow_instance_id: WorkflowInstanceId,
    pub expected_workflow_state_version: i32,
    pub transition_definition_id: TransitionId,
    pub context_payload: serde_json::Value,
    pub submission_payload: serde_json::Value,
}
```

冻结门禁：

1. 调用者必须同时等于 `created_by_principal_id` 和 current Visit assignee；
2. current source node 必须是 `DRAFT`；
3. 只允许执行该 DRAFT 的 `primary_advance_transition_id`，effect 必须为 `ADVANCE`；
4. Context 与 Submission payload 均必填，并分别通过大小和 Schema 校验；
5. PUBLISHED / DEPRECATED 可继续，REVOKED / DRAFT 拒绝。

固定事务顺序：

```text
CommandReceipt
→ WorkflowInstance FOR UPDATE
→ DefinitionVersion FOR UPDATE
→ validate version / permissions / DRAFT primary ADVANCE / schemas
→ insert new ContextRevision
→ insert Submission bound to the new ContextRevision
→ insert target NodeVisit
→ update both Instance pointers and workflowStateVersion +1
→ insert exactly one combined Event
→ complete Receipt
→ commit
```

新 Revision 与 Submission 必须满足：

```text
newRevision.previousRevisionId = pre-command currentContextRevisionId
submission.contextRevisionId = newRevision.contextRevisionId
```

Event 字段：

| Field | Value |
|---|---|
| `event_type` | `WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED` |
| `transition_effect` | `ADVANCE` |
| `source_node_visit_id` | 命令前 DRAFT Visit |
| `target_node_visit_id` | 新目标 Visit |
| `context_revision_id` | 新 Revision |
| `submission_id` | 新 Submission |
| `old_workflow_state_version` | N |
| `new_workflow_state_version` / `event_sequence` | N + 1 |

requestHash 使用与本合同第 13 节相同的 JCS 信封，command type 为
`REVISE_CONTEXT_AND_TRANSITION`，request body 同时包含
`context_payload` 与 `submission_payload`。成功响应为：

```json
{
  "workflowInstanceId": "uuid",
  "workflowStateVersion": 2,
  "currentContextRevisionId": "uuid",
  "sourceNodeVisitId": "uuid",
  "currentNodeVisitId": "uuid",
  "submissionId": "uuid",
  "eventSequence": 2
}
```

同 key/hash 重放同一响应；不同 key/same expectedVersion 只能一个成功。
revise-only、transition-only 与组合命令都锁同一 Instance，因此只能线性化成功一个。
本命令使用现有表和约束，不新增 Migration。

## 19. Not Implemented (not in scope)

- HTTP / gRPC / CLI routes
- Admin emergency override (separate PR)
- Reassign / Handoff
- Parallel nodes / conditional branching
