# svc-workflow Internal API Contract v0.1

Status: `FROZEN_FOR_STAGE_1_SMOKE`

This contract exposes a minimal cross-process adapter over the existing workflow application
services. It does not change the workflow domain, storage, event, receipt, or migration contracts.
It is intended for an isolated ADC integration smoke only, not Shadow or Cutover.

## Identity and authentication

Business endpoints require `Authorization: Bearer <jwt>`. Tokens are auth-service machine access
tokens and are verified with these rules:

- HS256 is the only accepted algorithm.
- `WORKFLOW_JWT_SECRET` is the shared auth-service signing secret and has no default.
- `iss` must equal `WORKFLOW_JWT_ISSUER` (default `auth-service`).
- `aud` must equal `WORKFLOW_JWT_AUDIENCE` (default `svc-workflow`).
- `sub`, `iss`, `aud`, `exp`, and `iat` are required.
- `principal_type=agent`, `type=access`, and `version=v1` are required.
- Clock skew defaults to 60 seconds (`WORKFLOW_JWT_CLOCK_SKEW`).

The verified `JWT.sub` must be a UUID and is used directly as the domain `PrincipalId`:

```text
JWT.sub = PrincipalId = command principal_id
```

The principal must already exist in `principals`. An unknown `JWT.sub` fails at the identity
mapping boundary with `404 principal_not_found` and does not create a command receipt because
`workflow_command_receipts.principal_id` has a principal foreign key. This is the only intentional
pre-receipt identity failure. A known but disabled principal owns a stable completed failure receipt.

Request bodies must not contain `principalId`, `actorPrincipalId`, or
`createdByPrincipalId`. All DTOs reject unknown fields, so attempted actor injection returns
`400 unknown_field`.

Write endpoints require `workflow.execute`; query endpoints require `workflow.read`. Missing or
invalid authentication returns 401. Missing scope returns 403.

## Endpoints

Unauthenticated service endpoints:

| Method | Path | Result |
|---|---|---|
| GET | `/healthz` | Process liveness only; no database access |
| GET | `/readyz` | Database probe plus exact successful migration ledger `0001..0010` |
| GET | `/version` | Service, kernel, build SHA, schema, and API contract versions |

Authenticated business endpoints:

| Method | Path | Scope | Idempotency |
|---|---|---|---|
| POST | `/internal/v1/workflow-instances` | `workflow.execute` | Required |
| GET | `/internal/v1/workflow-instances/{workflowInstanceId}` | `workflow.read` | n/a |
| POST | `/internal/v1/workflow-instances/{workflowInstanceId}/transitions` | `workflow.execute` | Required |
| GET | `/internal/v1/workflow-instances/{workflowInstanceId}/timeline` | `workflow.read` | n/a |

The adapter calls application services only. HTTP handlers must not issue workflow writes directly.

## Requests

JSON request fields use camelCase and strict deserialization.

Create:

```json
{
  "domainId": "uuid",
  "definitionVersionId": "uuid",
  "externalReference": "optional string",
  "externalUrl": "optional string",
  "metadata": {},
  "contextPayload": {}
}
```

Transition (the instance ID comes only from the route):

```json
{
  "transitionDefinitionId": "uuid",
  "expectedWorkflowStateVersion": 1,
  "submissionPayload": null
}
```

The adapter injects the route instance ID into the existing domain command. It does not alter the
domain request-hash envelope or add route parameters to the hash.

`externalReference`, when present, is limited to 512 Unicode characters. Longer values are
rejected by the adapter with `422 invalid_input` before the application service is called.

### Idempotency-Key

`Idempotency-Key` is required on both POST endpoints and is the only idempotency-key source. It
must contain 1 to 128 visible ASCII characters (`0x21..0x7e`) with no whitespace. The value is
passed unchanged to the domain receipt logic and is not part of `requestHash`.

The scope remains `(JWT.sub, Idempotency-Key)`. An exact replay returns the original semantic
result; reuse with different command content returns opaque `409 idempotency_conflict` without
the stored command ID or request hash.

After receipt ownership, all deterministic business validations that precede the first runtime
fact write complete and commit the failure receipt. This includes disabled principals, domain and
membership gates, definition and assignee gates, optimistic version checks, transition and return
reference checks, schema validation, and application payload size limits. Exact replay returns the
same persisted status and semantic error detail even if the underlying business state later changes.
Infrastructure failures and internal consistency failures roll back instead of becoming stable
business failure receipts.

## Responses

Create returns `201 Created`, a `Location` header, and camelCase result fields. Transition returns
200 and camelCase result fields.

Detail and timeline preserve the existing query projection's snake_case item fields. The detail
visibility discriminator is `full` or `historical_participant`. Timeline wraps its projection in:

```json
{
  "items": [],
  "nextCursor": null
}
```

Timeline pagination is keyset-based: `after` is an event sequence, `limit` defaults to 50 and must
be from 1 through 100. Invalid or malformed pagination returns `422 invalid_pagination` using the
standard error envelope.

## Error envelope

All adapter and domain errors are returned as JSON:

```json
{
  "error": {
    "code": "workflow_state_version_conflict",
    "message": "workflow state version does not match",
    "details": { "expected": 1, "actual": 2 }
  }
}
```

Storage and consistency details are redacted from responses. Storage/infrastructure failures return
`503 service_unavailable`; internal consistency failures remain `500 internal_consistency_error`.
Query not-found and not-visible states share `404 workflow_instance_not_found_or_not_visible`.
Idempotency conflicts are opaque.

## Runtime configuration

| Variable | Default |
|---|---|
| `DATABASE_URL` | Existing local PostgreSQL default |
| `WORKFLOW_BIND_ADDR` | `127.0.0.1` |
| `WORKFLOW_PORT` | `8989` |
| `WORKFLOW_JWT_SECRET` | Required, no default |
| `WORKFLOW_JWT_ISSUER` | `auth-service` |
| `WORKFLOW_JWT_AUDIENCE` | `svc-workflow` |
| `WORKFLOW_JWT_CLOCK_SKEW` | `60` seconds |
| `WORKFLOW_REQUEST_BODY_MAX_BYTES` | `2097152` |
| `WORKFLOW_REQUEST_TIMEOUT_SECS` | `30` |

Default startup validates configuration, applies migrations, starts Axum, and waits for SIGINT or
SIGTERM. `svc-workflow --migrate` applies migrations and exits without requiring JWT configuration.

## Explicit exclusions

V0 does not expose definition management, context revision, combined revise-and-transition,
administrative recovery, legacy import, worklists, Shadow Relay, Outbox, ADC write-back, delegated
actors, JWKS, or public network exposure.
