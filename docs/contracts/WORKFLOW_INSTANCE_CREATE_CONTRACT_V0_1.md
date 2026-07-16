# Workflow Instance Create — Implementation Contract v0.1

```text
Status: IMPLEMENTATION_CONTRACT
Version: v0.1
Architecture: SVC_WORKFLOW_ARCHITECTURE_V0_3_1 (ARCHITECTURE_FROZEN)
PR: 3A
```

## 1. Command Input

```rust
pub struct CreateWorkflowInstanceCommand {
    pub principal_id: PrincipalId,
    pub idempotency_key: String,
    pub command_schema_version: String,
    pub domain_id: DomainId,
    pub definition_version_id: DefinitionVersionId,
    pub external_reference: Option<String>,
    pub external_url: Option<String>,
    pub metadata: serde_json::Value,
    pub context_payload: serde_json::Value,
}
```

The following values are never accepted from the client — they are server-generated or resolved:
`workflow_instance_id`, `context_revision_id`, `node_visit_id`, `event_id`, `command_id`, `workflow_state_version`, `event_sequence`, `revision_number`, `visit_number`, `initial_node_id`, `resolved_assignee_principal_id`, `definition_digest`, `created_at`.

## 2. Authorization

1. **Principal**: Must exist and be `enabled = true`.
2. **Domain Membership**: Caller must have at least one active (`enabled = true`) binding in `domain_role_bindings` for the target domain. Any role key is sufficient — `DOMAIN_OWNER` is not required.
3. **Domain**: Must exist and be `enabled = true`.
4. **Cross-domain**: The definition version must belong to the specified domain. A principal who is owner of domain A cannot create instances in domain B using domain B's definitions, even if they are an owner of domain A.

## 3. Lock Order

The only row lock acquired is:

```
workflow_definition_versions WHERE definition_version_id = $1 FOR UPDATE
```

This is the same lock acquired by `atomic_publish`, `atomic_deprecate`, and `atomic_revoke` in the Definition Service. All operations lock a single row, so no deadlock cycle exists.

## 4. Transaction Steps

```
BEGIN

  1. INSERT INTO workflow_command_receipts ON CONFLICT (principal_id, idempotency_key) DO NOTHING RETURNING command_id
     If receipt already exists → branch to idempotent replay (§6)

  2. SELECT ... FROM workflow_definition_versions WHERE id = $1 FOR UPDATE
     Lock the version row

  3. SELECT domain_id FROM workflow_definitions WHERE id = $1
     Verify version belongs to the input domain_id

  4. SELECT enabled FROM domains WHERE domain_id = $1
     Verify domain exists and is enabled

  5. SELECT enabled FROM principals WHERE principal_id = $1
     Verify principal exists and is enabled (re-check inside tx)

  6. SELECT 1 FROM domain_role_bindings WHERE domain_id = $1 AND principal_id = $2 AND enabled = TRUE LIMIT 1
     Verify domain membership

  7. Verify version status = PUBLISHED

  8. SELECT node_id, assignee_ref_type, fixed_principal_id FROM workflow_node_definitions
     WHERE definition_version_id = $1 AND node_type = 'DRAFT'
     Read the unique DRAFT node

  9. Resolve assignee (§5)

  10. Validate context_payload against context_schema (§7)

  11. INSERT INTO workflow_instances (with current_context_revision_id and current_node_visit_id
      pointing to IDs inserted in steps 12-13 — DEFERRED FK)

  12. INSERT INTO workflow_context_revisions (revision_number = 1, previous_revision_id = NULL)

  13. INSERT INTO workflow_node_visits (visit_number = 1, entered_by_transition_id = NULL)

  14. INSERT INTO workflow_events (INSTANCE_CREATED, event_sequence = 1)

  15. UPDATE workflow_command_receipts SET receipt_status = 'COMPLETED',
      response_status, response_body, response_digest

COMMIT
```

**Circular FK resolution**: `workflow_instances.current_context_revision_id` and `current_node_visit_id` reference `workflow_context_revisions` and `workflow_node_visits` respectively, which themselves reference `workflow_instances`. These FKs are `DEFERRABLE INITIALLY DEFERRED`. All three rows are inserted in the same transaction, so the FK constraints are satisfied at commit time.

## 5. Assignee Resolution

### WORKFLOW_CREATOR
The command principal becomes the initial assignee:
```
resolved_assignee_principal_id = command.principal_id
```

### DOMAIN_OWNER
Query the single enabled DOMAIN_OWNER for the target domain:
```sql
SELECT principal_id FROM domain_role_bindings
WHERE domain_id = $1 AND role_key = 'DOMAIN_OWNER' AND enabled = TRUE
LIMIT 1
```
Then verify the owner principal exists and is `enabled = true`.

### FIXED_PRINCIPAL
Use the `fixed_principal_id` stored in the DRAFT node's `assignee_ref`. Verify the principal exists and is `enabled = true`.

If resolution fails at any step (not found, disabled, missing fixed_principal_id, no enabled DOMAIN_OWNER), the entire creation fails atomically with `AssigneeResolutionFailed`.

## 6. Idempotency

### 6.1 Request Hash Computation

The request hash is computed over the canonical JCS-normalized request envelope.

**Field naming**: The Rust structs use `snake_case` field names (via `#[derive(Serialize)]` without rename attributes). The canonical JSON keys match the code, not earlier documentation versions.

**Canonical structure**:

```json
JCS({
  "command_schema_version": "v1",
  "command_type": "CREATE_WORKFLOW_INSTANCE",
  "route_parameters": {},
  "request_body": {
    "principal_id": "<uuid>",
    "domain_id": "<uuid>",
    "definition_version_id": "<uuid>",
    "context_payload": { ... },
    "metadata": { ... },
    "external_reference": null,
    "external_url": null
  }
}) → SHA-256
```

**Key rules**:
| Property | Value |
|---|---|
| `command_schema_version` | From command input (e.g. `"v1"`) |
| `command_type` | Constant `"CREATE_WORKFLOW_INSTANCE"` |
| `route_parameters` | Stable `{}` (no HTTP route) |
| `request_body` | Nested object containing all command fields except `idempotency_key` |
| `null` fields | `Option::None` serializes as JSON `null` (both `external_reference` and `external_url`) |
| Object key order | JCS sorts all object keys alphabetically |
| Hash function | `jcs_canonicalize::sha256_jcs_hex` (JCS canonicalization + SHA-256) |

The idempotency key itself is excluded from the hash.

**Golden test**: A deterministic golden test at `tests/17_workflow_instance_create/request_hash_contract.rs` fixes both the canonical JSON and the SHA-256 hex output. For a fixed input with UUIDs `11111111-...`/`22222222-...`/`33333333-...`, the golden values are:

```text
Canonical JSON (JCS-sorted):
{"command_schema_version":"v1","command_type":"CREATE_WORKFLOW_INSTANCE","request_body":{"context_payload":{"hello":"world"},"definition_version_id":"33333333-3333-3333-3333-333333333333","domain_id":"22222222-2222-2222-2222-222222222222","external_reference":null,"external_url":null,"metadata":{"source":"test"},"principal_id":"11111111-1111-1111-1111-111111111111"},"route_parameters":{}}

SHA-256: ba40a90a5227ae7608f36e0bc2f0ca21092e1a3e56d5380f93655693b55a0d97
```

Changing any field name, null semantics, or envelope structure will cause the golden test to fail.

### 6.2 First Request

```sql
INSERT INTO workflow_command_receipts (...)
VALUES (...)
ON CONFLICT (principal_id, idempotency_key) DO NOTHING
RETURNING command_id
```

If a row is returned, this transaction owns the request. Proceed with creation.

### 6.3 Existing Idempotency Key

If INSERT returns no row:

```sql
SELECT ... FROM workflow_command_receipts
WHERE principal_id = $1 AND idempotency_key = $2
FOR UPDATE
```

**Same request_hash, COMPLETED**: Replay the stored response body. No second instance, event, or state version is created. The stored response is returned as-is (extracting instance IDs from the JSON response).

**Different request_hash**: Write a `workflow_command_attempt_audits` entry with `attempt_type = 'IDEMPOTENCY_CONFLICT'`. Return `IdempotencyConflict` with the original `command_id` and `request_hash`. The original receipt is never modified.

**Same request_hash, PROCESSING**: Return `CommandStillProcessing`. Never take over or modify the original command.

### 6.4 Deterministic Failure Replay

Deterministic business failures (disabled principal, disabled domain, invalid context, etc.) are persisted as COMPLETED receipts with an error response body. Replaying the same idempotent request returns the same error response, without re-running creation logic.

The deterministic failure sequence for all validated checks:
1. Insert PROCESSING receipt (step 1 of the transaction)
2. Validate business rules
3. On failure: complete the receipt with an error response (status + error code)
4. Commit the transaction — the COMPLETED receipt persists, no runtime facts created
5. On replay: the existing COMPLETED receipt is detected → same error response returned

## 7. Context Validation

### Schema Validation

If the definition version has a `context_schema`:
1. Compile the schema using `jsonschema::validator_for`
2. Validate `context_payload` against the compiled schema
3. Any validation error returns `ContextValidationFailed`

If `context_schema` is `None`, any valid JSON is accepted (subject to size limits).

The schema validator is the same `jsonschema` 0.47 crate used by the Definition Service. External `$ref` resolution is not performed — the schema is already validated at publish time to contain only local fragment references.

### Service-Layer Size Limits

| Field | Limit | Check Method |
|---|---|---|
| `context_payload` | 1 MiB | `serde_json::to_vec` → `.len()` |
| `metadata` | 64 KiB | `serde_json::to_vec` → `.len()` |

### Database Size Limits (defense in depth)

| Table / Column | Limit | Mechanism |
|---|---|---|
| `workflow_context_revisions.payload` | 1 MiB | `chk_ctx_payload_size` (pg_column_size) |
| `workflow_instances.metadata` | 64 KiB | `chk_instance_metadata_size` (pg_column_size) |

Service-layer limits are checked on raw serialized bytes before JSONB encoding. Database limits are checked on PostgreSQL's JSONB binary storage size, which may differ slightly due to type overhead.

## 8. Event Field Matrix

| Field | Value |
|---|---|
| `event_type` | `INSTANCE_CREATED` |
| `source_node_visit_id` | NULL |
| `target_node_visit_id` | Initial NodeVisit ID |
| `context_revision_id` | Context Revision #1 ID |
| `submission_id` | NULL |
| `before_workflow_state_version` | 0 |
| `after_workflow_state_version` | 1 |
| `event_sequence` | 1 |
| `actor_principal_id` | Caller principal |
| `command_id` | Current Receipt command_id |
| `event_schema_version` | `"v1"` |

### Event Data

```json
{
  "definitionVersionId": "...",
  "definitionDigest": "...",
  "initialNodeId": "...",
  "assigneeResolutionType": "WORKFLOW_CREATOR"
}
```

`event_data_digest` = JCS(event_data) → SHA-256

The full context payload is never duplicated in the event data.

## 9. Success Response

```json
{
  "workflowInstanceId": "...",
  "workflowStateVersion": 1,
  "currentContextRevisionId": "...",
  "currentNodeVisitId": "...",
  "eventSequence": 1
}
```

`response_digest` = JCS(response) → SHA-256. The response is stable and persisted — idempotent replay returns the exact same response body.

## 10. Deterministic Failure vs Infrastructure Failure

### Deterministic (persisted as COMPLETED)

Deterministic business failures are caught inside the transaction, the receipt is completed with an error response, and the transaction is committed. The COMPLETED receipt (with error response) persists. No runtime facts (instance, context, visit, event) are created.

| Condition | Status Code | Error Code |
|---|---|---|
| Domain not found | 404 | `domain_not_found` |
| Domain disabled | 403 | `domain_disabled` |
| Principal not found | 404 | `principal_not_found` |
| Principal disabled | 403 | `principal_disabled` |
| No domain membership | 403 | `domain_membership_required` |
| Cross-domain violation | 403 | `cross_domain_violation` |
| Version not found | 404 | `definition_version_not_found` |
| Version not PUBLISHED | 409 | `version_not_published` |
| Context validation failed | 422 | `context_validation_failed` |
| Size limit exceeded | 413 | `size_limit_exceeded` |
| Assignee resolution failed | 422 | `assignee_resolution_failed` |

### Infrastructure (transaction rolls back)

Infrastructure failures cause the entire PostgreSQL transaction to roll back. This includes the PROCESSING receipt that was inserted at step 1 — no receipt of any kind persists. The caller receives no stored response and must retry.

- Connection drops
- Database unavailable
- Serialization failures
- Unknown SQL errors
- Trigger-enforced constraint violations (used for fault injection testing)

**Important**: The initial PROCESSING receipt is always rolled back on infrastructure failure. There is no scenario where an infrastructure failure leaves a residual receipt.

## 11. Migration

Migration 0009 adds `external_reference TEXT` to `workflow_instances`:
```sql
ALTER TABLE workflow_instances
    ADD COLUMN IF NOT EXISTS external_reference TEXT
    CHECK (external_reference IS NULL OR char_length(external_reference) <= 512);
```

All other required tables (workflow_instances, workflow_context_revisions, workflow_node_visits, workflow_events, workflow_command_receipts, workflow_command_attempt_audits) already exist in migrations 0003-0006.

## 12. Limitations (not implemented in this PR)

- Context Revision #2+
- Context modification (revise)
- Submission / Transition / RETURN / TERMINATE
- Admin emergency override
- HTTP / gRPC / CLI
- Timer / Signal / Reassign / Subject / Parallel workflow
- Cross-instance references

## 13. Test Coverage Mapping (40-Item)

| # | Requirement | Test(s) | File |
|---|---|---|---|
| 1–8 | Normal creation (8 scenarios) | `test_create_success_wf_creator`, `test_create_success_domain_owner_assignee`, `test_create_success_fixed_principal_assignee`, `test_create_all_records_present`, `test_create_current_pointers_correct`, `test_create_event_field_matrix_correct`, `test_create_context_digest_readback`, `test_create_response_digest_readback` | `normal_create.rs` |
| 9 | Draft version rejected | `test_draft_version_rejected` | `definition_gates.rs` |
| 10 | Deprecated version rejected | `test_deprecated_version_rejected` | `definition_gates.rs` |
| 11 | Revoked version rejected | `test_revoked_version_rejected` | `definition_gates.rs` |
| 12 | Cross-domain version rejected | `test_cross_domain_version_rejected` | `definition_gates.rs` |
| 13 | Domain disabled rejected | `test_disabled_domain_rejected` | `definition_gates.rs` |
| 14 | No DRAFT node defensive | `test_no_draft_node_defensive_failure` | `definition_gates.rs` |
| 15 | No domain membership rejected | `test_no_domain_membership_rejected` | `authorization.rs` |
| 16 | Principal disabled rejected | `test_disabled_principal_rejected` | `authorization.rs` |
| 17 | Domain owner assignee resolved | `test_create_success_domain_owner_assignee` | `normal_create.rs` |
| 18 | Fixed principal assignee resolved | `test_create_success_fixed_principal_assignee` | `normal_create.rs` |
| 19 | Disabled domain owner assignee rejected | `test_disabled_domain_owner_assignee_rejected` | `authorization.rs` |
| 20 | Any valid context accepted | `test_valid_context_accepted` | `context_validation.rs` |
| 21 | Non-null schema: valid payload | `test_context_schema_valid_accepted` | `context_validation.rs` |
| 22 | Non-null schema: required field missing | `test_context_schema_required_field_missing` | `context_validation.rs` |
| 23 | Non-null schema: type error | `test_context_schema_type_error_rejected` | `context_validation.rs` |
| 24 | Non-null schema: additional properties rejected & local `$ref` | `test_context_schema_additional_properties_rejected`, `test_context_schema_local_ref_accepted` | `context_validation.rs` |
| 25 | Same key, same request → same instance | `test_same_key_same_request_returns_same_instance` | `idempotency.rs` |
| 26 | Replay does not create second event | `test_replay_does_not_create_second_event` | `idempotency.rs` |
| 27 | Different request, same key → conflict | `test_different_request_same_key_conflict` | `idempotency.rs` |
| 28 | Conflict writes attempt audit | `test_conflict_writes_attempt_audit` | `idempotency.rs` |
| 29 | Conflict does not modify original receipt | `test_conflict_does_not_modify_original_receipt` | `idempotency.rs` |
| 30 | Different principal, same key → allowed | `test_different_principal_same_key_allowed` | `idempotency.rs` |
| 31 | Deterministic failure replayable | `test_deterministic_failure_replayable` | `idempotency.rs` |
| 32 | Concurrent same idempotent request | `test_concurrent_same_idempotent_request` | `idempotency.rs` |
| 33 | Concurrent different request hash | `test_concurrent_different_request_hash` | `idempotency.rs` |
| 34 | PROCESSING receipt not taken over | `test_processing_receipt_not_taken_over` | `idempotency.rs` |
| 35 | Deterministic failure → no runtime facts | `test_deterministic_failure_no_runtime_facts_left` | `atomicity.rs` |
| 36 | Infrastructure failure → no residual receipt | `test_infrastructure_failure_no_residual_receipt` | `atomicity.rs` |
| 37 | Receipt completion failure → all facts roll back | `test_receipt_completion_failure_rolls_back_all_runtime_facts` | `atomicity.rs` |
| 38 | Exactly one event per creation | `test_exactly_one_event_per_creation` | `atomicity.rs` |
| 39 | Event field matrix correct | `test_create_event_field_matrix_correct` | `normal_create.rs` |
| 40 | Context digest readback | `test_create_context_digest_readback` | `normal_create.rs` |

**新增测试**：
- `test_context_schema_valid_accepted` — H2 非空 Schema 合法 Context
- `test_context_schema_required_field_missing` — H2 缺少 required 字段拒绝
- `test_context_schema_type_error_rejected` — H2 类型错误拒绝
- `test_context_schema_additional_properties_rejected` — H2 额外属性拒绝
- `test_context_schema_local_ref_accepted` — H2 本地 `$ref` 可用
- `test_receipt_completion_failure_rolls_back_all_runtime_facts` — M3 Receipt 完成失败独立测试
- `test_request_hash_golden_canonical_json` / `test_request_hash_golden_sha256` — M2 requestHash Golden Test
