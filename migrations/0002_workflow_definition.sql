-- Migration 0002: Workflow Definition tables
-- Defines templates, versions, nodes, and transitions

-- ============================================================
-- Workflow Definitions
-- ============================================================

CREATE TABLE workflow_definitions (
    workflow_definition_id  UUID        NOT NULL PRIMARY KEY,
    domain_id               UUID        NOT NULL REFERENCES domains(domain_id),
    definition_key          TEXT        NOT NULL CHECK (char_length(definition_key) >= 1 AND char_length(definition_key) <= 128),
    display_name            TEXT        NOT NULL CHECK (char_length(display_name) >= 1 AND char_length(display_name) <= 256),
    description             TEXT,
    metadata                JSONB,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Each definition_key is unique within a domain
CREATE UNIQUE INDEX idx_wf_def_domain_key
    ON workflow_definitions (domain_id, definition_key);

CREATE INDEX idx_wf_def_domain ON workflow_definitions (domain_id);

-- ============================================================
-- Workflow Definition Versions
-- ============================================================

CREATE TABLE workflow_definition_versions (
    definition_version_id   UUID                        NOT NULL PRIMARY KEY,
    workflow_definition_id  UUID                        NOT NULL REFERENCES workflow_definitions(workflow_definition_id),
    version_number          INTEGER                     NOT NULL CHECK (version_number >= 1),
    version_status          definition_version_status   NOT NULL DEFAULT 'DRAFT',

    -- Digest: SHA-256 hex string (64 hex chars)
    definition_digest       TEXT                        CHECK (definition_digest IS NULL OR definition_digest ~ '^[0-9a-f]{64}$'),

    -- JSON Schema dialect and version
    json_schema_dialect     TEXT                        CHECK (json_schema_dialect IS NULL OR char_length(json_schema_dialect) <= 256),
    validator_version       TEXT                        CHECK (validator_version IS NULL OR char_length(validator_version) <= 64),

    -- Context and submission schemas
    context_schema          JSONB,
    submission_schema       JSONB,

    metadata                JSONB,
    created_at              TIMESTAMPTZ                 NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ                 NOT NULL DEFAULT now(),

    -- Lifecycle tracking
    published_at            TIMESTAMPTZ,
    deprecated_at           TIMESTAMPTZ,
    revoked_at              TIMESTAMPTZ
);

-- Version numbers are unique within a definition
CREATE UNIQUE INDEX idx_wf_def_ver_number
    ON workflow_definition_versions (workflow_definition_id, version_number);

-- ============================================================
-- Workflow Node Definitions
-- ============================================================

CREATE TABLE workflow_node_definitions (
    node_id                     UUID                NOT NULL PRIMARY KEY,
    definition_version_id       UUID                NOT NULL REFERENCES workflow_definition_versions(definition_version_id),
    node_key                    TEXT                NOT NULL CHECK (char_length(node_key) >= 1 AND char_length(node_key) <= 128),
    display_name                TEXT                NOT NULL CHECK (char_length(display_name) >= 1 AND char_length(display_name) <= 256),
    order_index                 INTEGER             NOT NULL CHECK (order_index >= 0),
    node_type                   node_type           NOT NULL,
    assignee_ref_type           assignee_ref_type   NOT NULL,
    fixed_principal_id          UUID                REFERENCES principals(principal_id),
    instructions                TEXT                CHECK (instructions IS NULL OR char_length(instructions) <= 10000),
    primary_advance_transition_id UUID,
    metadata                    JSONB,
    created_at                  TIMESTAMPTZ         NOT NULL DEFAULT now()
);

-- Constraint: FIXED_PRINCIPAL requires fixed_principal_id
-- Other types must have null fixed_principal_id
ALTER TABLE workflow_node_definitions
    ADD CONSTRAINT chk_fixed_principal
    CHECK (
        (assignee_ref_type = 'FIXED_PRINCIPAL' AND fixed_principal_id IS NOT NULL)
        OR
        (assignee_ref_type IN ('WORKFLOW_CREATOR', 'DOMAIN_OWNER') AND fixed_principal_id IS NULL)
    );

-- Node keys are unique within a definition version
CREATE UNIQUE INDEX idx_wf_node_def_ver_key
    ON workflow_node_definitions (definition_version_id, node_key);

-- Order indices are unique within a definition version
CREATE UNIQUE INDEX idx_wf_node_def_ver_order
    ON workflow_node_definitions (definition_version_id, order_index);

CREATE INDEX idx_wf_node_def_ver ON workflow_node_definitions (definition_version_id);

-- ============================================================
-- Workflow Transition Definitions
-- ============================================================

CREATE TABLE workflow_transition_definitions (
    transition_id               UUID                    NOT NULL PRIMARY KEY,
    definition_version_id       UUID                    NOT NULL REFERENCES workflow_definition_versions(definition_version_id),
    transition_key              TEXT                    NOT NULL CHECK (char_length(transition_key) >= 1 AND char_length(transition_key) <= 128),
    display_name                TEXT                    NOT NULL CHECK (char_length(display_name) >= 1 AND char_length(display_name) <= 256),
    source_node_id              UUID                    NOT NULL REFERENCES workflow_node_definitions(node_id),
    target_node_id              UUID                    NOT NULL REFERENCES workflow_node_definitions(node_id),
    transition_effect           transition_effect       NOT NULL,
    submission_schema           JSONB,
    metadata                    JSONB,
    created_at                  TIMESTAMPTZ             NOT NULL DEFAULT now(),

    -- Transition keys are unique within a definition version
    UNIQUE (definition_version_id, transition_key),
    -- Composite unique needed for FK references by (transition_id, definition_version_id)
    UNIQUE (transition_id, definition_version_id)
);

CREATE INDEX idx_wf_trans_def_ver ON workflow_transition_definitions (definition_version_id);
CREATE INDEX idx_wf_trans_source ON workflow_transition_definitions (source_node_id);
CREATE INDEX idx_wf_trans_target ON workflow_transition_definitions (target_node_id);

-- Add the foreign key from node_definitions to its primary advance transition
-- This is deferred because the transition may not exist yet when the node is created
ALTER TABLE workflow_node_definitions
    ADD CONSTRAINT fk_primary_advance_transition
    FOREIGN KEY (primary_advance_transition_id, definition_version_id)
    REFERENCES workflow_transition_definitions (transition_id, definition_version_id)
    DEFERRABLE INITIALLY DEFERRED;
