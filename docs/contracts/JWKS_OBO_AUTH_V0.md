# svc-workflow JWKS / OBO Auth Contract v0

```text
Status: FROZEN_FOR_STAGE_1_AUTHENTICATED_SMOKE
```

This contract describes the dual-mode authentication layer added to
`svc-workflow` for Stage 1 authenticated smoke. It does **not** describe
Principal Provisioning, Domain/Role Provisioning, persistent delegation audit,
or production deployment.

---

## 1. Dual auth modes

The server picks one mode via `WORKFLOW_AUTH_MODE`:

| Mode         | Algorithm | Key source     | Environment        |
|--------------|-----------|----------------|--------------------|
| `test_hs256` | HS256     | Shared secret  | Local, loopback    |
| `jwks`       | RS256     | JWKS endpoint  | Formal, Canary, Shadow, Cutover |

### test_hs256 gates

- `WORKFLOW_JWT_SECRET` is required and must be non-empty.
- `WORKFLOW_JWKS_URL` must **not** be set.
- Server must bind to a loopback address (`127.0.0.1`).

### jwks gates

- `WORKFLOW_JWKS_URL`, `WORKFLOW_JWT_ISSUER`, `WORKFLOW_JWT_AUDIENCE` are all required.
- `WORKFLOW_JWT_SECRET` must **not** be set (no HS256 fallback).
- No loopback restriction.

Missing required configuration causes a startup failure with a clear error
message. Conflicting configuration (e.g. both secret and JWKS URL) also fails.

---

## 2. JWKS configuration

| Variable                       | Default | Required | Purpose                     |
|--------------------------------|---------|----------|-----------------------------|
| `WORKFLOW_AUTH_MODE`           | —       | Yes      | `test_hs256` or `jwks`      |
| `WORKFLOW_JWKS_URL`            | —       | Yes*     | JWKS endpoint URL           |
| `WORKFLOW_JWT_ISSUER`          | `auth-service` | Yes* | Expected `iss` claim        |
| `WORKFLOW_JWT_AUDIENCE`        | `svc-workflow` | Yes* | Expected `aud` claim        |
| `WORKFLOW_JWKS_CACHE_TTL`      | `300`   | No       | Cache TTL (seconds)         |
| `WORKFLOW_JWKS_HTTP_TIMEOUT`   | `5`     | No       | JWKS fetch timeout          |
| `WORKFLOW_JWKS_MAX_STALE`      | `600`   | No       | Max stale window (seconds)  |
| `WORKFLOW_JWT_CLOCK_SKEW`      | `60`    | No       | Clock skew tolerance        |

*Required in `jwks` mode only.

---

## 3. Token claims

### Direct access token

```json
{
  "token_use": "access",
  "sub": "<actor UUID>",
  "principal_type": "human | agent",
  "iss": "auth-service",
  "aud": "svc-workflow",
  "exp": 1700000000,
  "type": "access",
  "version": "v1",
  "scope": "workflow.execute workflow.read"
}
```

### OBO (on-behalf-of) token

```json
{
  "token_use": "workflow_obo",
  "sub": "<actual actor UUID>",
  "principal_type": "human | agent",
  "iss": "auth-service",
  "aud": "svc-workflow",
  "exp": 1700000000,
  "type": "access",
  "version": "v1",
  "scope": "workflow.execute workflow.read",
  "act": {
    "sub": "<ADC service MachinePrincipal UUID>"
  },
  "azp": "<ADC OAuth client ID>",
  "jti": "<unique token identifier>"
}
```

### Required claims (all tokens)

| Claim            | Validation                                      |
|------------------|-------------------------------------------------|
| `sub`            | Must be a valid UUID                            |
| `iss`            | Must match `WORKFLOW_JWT_ISSUER`                |
| `aud`            | Must match `WORKFLOW_JWT_AUDIENCE`              |
| `exp`            | Must be in the future (leeway applied)          |
| `iat`            | Must be present                                 |
| `type`           | Must be `access` (backward compat)              |
| `version`        | Must be `v1` (backward compat)                  |
| `principal_type` | Must be `human` or `agent`                      |
| `scope`          | Space-separated; read `workflow.read` / write `workflow.execute` |

### OBO-specific validations

| Claim     | Validation                        |
|-----------|-----------------------------------|
| `act.sub` | Must be a valid UUID              |
| `azp`     | Must be non-empty                 |
| `jti`     | Must be non-empty                 |

### Actor model

The domain actor for all authorization decisions is **always** `sub`:

```text
JWT.sub = PrincipalId = command principal_id
```

`act.sub` is used **only** for audit logging. It does **not**:

- Become the domain `principal_id`;
- Grant any authorization rights;
- Bypass assignee checks, domain membership checks, or scope checks.

---

## 4. Scope

The authentication layer enforces endpoint-level scope requirements:

| Endpoint(s)                       | Required scope      |
|-----------------------------------|---------------------|
| `POST .../workflow-instances`     | `workflow.execute`  |
| `POST .../transitions`            | `workflow.execute`  |
| `GET .../workflow-instances/{id}` | `workflow.read`     |
| `GET .../timeline`                | `workflow.read`     |

Missing scope returns `403 insufficient_scope`.

Scope enforcement at the authentication layer **does not** replace domain-level
authorization (assignee checks, domain membership, principal enabled/disabled,
command permissions). OBO delegation does **not** elevate scope.

---

## 5. JWKS fetch and cache

### Cache behavior

1. **First request** or **empty cache**: triggers a JWKS fetch.
2. **Within TTL** (`WORKFLOW_JWKS_CACHE_TTL`): cached keys are used directly.
3. **Beyond TTL, within max_stale** (`WORKFLOW_JWKS_MAX_STALE`):
   - Known `kid` → use cached key (no refresh).
   - Unknown `kid` → trigger a controlled refresh.
4. **Beyond max_stale**: evict cache, force refresh.

### Refresh concurrency suppression

Multiple concurrent requests encountering the same unknown `kid` are serialized
by a `Mutex`. Only one fetch proceeds; all others wait and then retry against
the updated cache.

### Fail-closed semantics

| Scenario                                      | Result     |
|-----------------------------------------------|------------|
| No cache + fetch fails                        | `503`      |
| Stale cache + known kid + fetch fails         | `200`       |
| Stale cache + unknown kid + fetch fails       | `401`       |
| Token signed with unknown `kid` after refresh | `401`       |

### JWKS key filtering

Only keys with these properties are accepted:

- `kty=RSA`
- `use=sig` or absent (compatible null)
- `alg=RS256` or absent
- `kid` present and non-empty
- `n` and `e` present

Invalid, duplicate, or malformed keys are silently skipped with a warning log.

### Security rules

- No private key material is accepted.
- Full JWKS or JWT is never written to normal logs.
- Network errors, JWKS URL, and JWKS internal state are never returned in error responses.

---

## 6. RS256 verification

- Only RS256 is accepted. HS256, `alg=none`, or any other algorithm is rejected.
- JWT must include a `kid` header.
- Signature is verified using the `jsonwebtoken` crate (no custom RSA implementation).
- Claims validation covers: `iss`, `aud`, `exp`, `nbf`, `sub`, `principal_type`, `token_use`, `scope`.

---

## 7. readyz

### test_hs256 mode

- Secret config already validated at startup. Always passes unless DB/migration fails.

### jwks mode

Verifier checks:

- Configuration is complete (validated at startup).
- At least one JWKS key was successfully fetched and cached.
- Cache is within the max-stale window.

Failure returns `503 auth_verifier_unavailable` without internal details.

---

## 8. Error envelope

Errors use the existing `{ "error": { "code", "message", "details?" } }` envelope:

| HTTP | `code`                      | Meaning                         |
|------|-----------------------------|---------------------------------|
| 401  | `invalid_token`             | Malformed, wrong alg, bad claims|
| 401  | `token_expired`             | Token is past `exp`             |
| 401  | `missing_claim`             | Required claim is absent        |
| 403  | `insufficient_scope`        | Scope `workflow.read/execute` missing |
| 503  | `auth_verifier_unavailable` | JWKS temporarily unavailable    |

Detailed cryptographic or network errors are never returned to clients.

---

## 9. Structured audit logging

Each authenticated request writes a structured log line:

```text
request_id, jti, sub, principal_type, token_use,
act.sub (or "-"), azp (or "-"), audience, scope,
endpoint, result
```

**Never logged:**

- Full JWT
- JWT signature
- Authorization header
- Full AuthContext
- Full Submission/Context payload
- Secrets

This log is sufficient for Stage 1 authenticated smoke. A persistent audit
solution must be designed before Write Shadow.

---

## 10. Not implemented (this version)

- Principal Provisioning API (`/internal/v1/admin/principals`)
- Domain / Role Provisioning API
- ADC OAuth Client integration
- auth-service Token Exchange
- Persistent delegation audit (database migration)
- Shadow / Outbox / Relay
- Production deployment
