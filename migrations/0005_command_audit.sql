-- Migration 0005: Command & Audit tables
-- Command receipts, attempt audits, and security audits

-- ============================================================
-- Workflow Command Receipts
-- ============================================================

CREATE TABLE workflow_command_receipts (
    command_id          UUID            NOT NULL PRIMARY KEY,
    principal_id        UUID            NOT NULL REFERENCES principals(principal_id),
    idempotency_key     TEXT            NOT NULL CHECK (char_length(idempotency_key) >= 1 AND char_length(idempotency_key) <= 512),
    command_type        TEXT            NOT NULL CHECK (char_length(command_type) >= 1 AND char_length(command_type) <= 128),

    -- SHA-256 hex string of the serialized command envelope
    request_hash        TEXT            NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),

    receipt_status      receipt_status  NOT NULL DEFAULT 'PROCESSING',

    response_status     INTEGER,
    response_body       JSONB,
    response_digest     TEXT            CHECK (response_digest IS NULL OR response_digest ~ '^[0-9a-f]{64}$'),

    created_at          TIMESTAMPTZ     NOT NULL DEFAULT now(),
    completed_at        TIMESTAMPTZ
);

-- (principal_id, idempotency_key) is unique
CREATE UNIQUE INDEX idx_wf_receipt_idempotency
    ON workflow_command_receipts (principal_id, idempotency_key);

CREATE INDEX idx_wf_receipt_principal ON workflow_command_receipts (principal_id);
CREATE INDEX idx_wf_receipt_status ON workflow_command_receipts (receipt_status);

-- ============================================================
-- Workflow Command Attempt Audits
-- ============================================================

CREATE TABLE workflow_command_attempt_audits (
    audit_id            UUID            NOT NULL PRIMARY KEY,
    command_id          UUID            NOT NULL REFERENCES workflow_command_receipts(command_id),
    principal_id        UUID            NOT NULL REFERENCES principals(principal_id),
    idempotency_key     TEXT            NOT NULL CHECK (char_length(idempotency_key) >= 1 AND char_length(idempotency_key) <= 512),
    attempt_type        TEXT            NOT NULL CHECK (char_length(attempt_type) >= 1 AND char_length(attempt_type) <= 128),
    failure_reason      TEXT            CHECK (failure_reason IS NULL OR char_length(failure_reason) <= 2000),
    request_hash        TEXT            NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    details             JSONB,
    created_at          TIMESTAMPTZ     NOT NULL DEFAULT now()
);

CREATE INDEX idx_wf_attempt_cmd ON workflow_command_attempt_audits (command_id);
CREATE INDEX idx_wf_attempt_principal ON workflow_command_attempt_audits (principal_id);

-- ============================================================
-- Workflow Security Audits
-- ============================================================

CREATE TABLE workflow_security_audits (
    audit_id            UUID            NOT NULL PRIMARY KEY,
    principal_id        UUID            NOT NULL REFERENCES principals(principal_id),
    action              TEXT            NOT NULL CHECK (char_length(action) >= 1 AND char_length(action) <= 128),
    resource_type       TEXT            CHECK (resource_type IS NULL OR char_length(resource_type) <= 128),
    resource_id         TEXT            CHECK (resource_id IS NULL OR char_length(resource_id) <= 256),
    details             JSONB,
    created_at          TIMESTAMPTZ     NOT NULL DEFAULT now()
);

CREATE INDEX idx_wf_sec_audit_principal ON workflow_security_audits (principal_id);
CREATE INDEX idx_wf_sec_audit_action ON workflow_security_audits (action);
CREATE INDEX idx_wf_sec_audit_created ON workflow_security_audits (created_at);
