-- Migration 0003: Runtime tables
-- Workflow instances, context revisions, node visits, and submissions

-- ============================================================
-- Workflow Instances
-- ============================================================

CREATE TABLE workflow_instances (
    workflow_instance_id        UUID        NOT NULL PRIMARY KEY,
    domain_id                   UUID        NOT NULL REFERENCES domains(domain_id),
    definition_version_id       UUID        NOT NULL REFERENCES workflow_definition_versions(definition_version_id),
    created_by_principal_id     UUID        NOT NULL REFERENCES principals(principal_id),

    -- Projections (updatable by command service)
    current_context_revision_id UUID,
    current_node_visit_id       UUID,
    workflow_state_version      INTEGER     NOT NULL DEFAULT 1 CHECK (workflow_state_version >= 1),

    -- Optional display fields
    external_url                TEXT        CHECK (external_url IS NULL OR char_length(external_url) <= 2048),
    metadata                    JSONB,

    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Domain + definition version index
CREATE INDEX idx_wf_inst_domain ON workflow_instances (domain_id);
CREATE INDEX idx_wf_inst_def_ver ON workflow_instances (definition_version_id);
CREATE INDEX idx_wf_inst_creator ON workflow_instances (created_by_principal_id);

-- ============================================================
-- Workflow Context Revisions
-- ============================================================

CREATE TABLE workflow_context_revisions (
    context_revision_id     UUID        NOT NULL PRIMARY KEY,
    workflow_instance_id    UUID        NOT NULL REFERENCES workflow_instances(workflow_instance_id),
    revision_number         INTEGER     NOT NULL CHECK (revision_number >= 1),
    previous_revision_id    UUID,
    payload                 JSONB       NOT NULL,
    payload_digest          TEXT        NOT NULL CHECK (payload_digest ~ '^[0-9a-f]{64}$'),
    created_by_principal_id UUID        NOT NULL REFERENCES principals(principal_id),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- (workflow_instance_id, revision_number) is unique
    UNIQUE (workflow_instance_id, revision_number),
    -- Composite unique for FK references
    UNIQUE (context_revision_id, workflow_instance_id)
);

-- Composite FK: previous_revision_id must belong to same workflow_instance
ALTER TABLE workflow_context_revisions
    ADD CONSTRAINT fk_previous_revision
    FOREIGN KEY (previous_revision_id, workflow_instance_id)
    REFERENCES workflow_context_revisions (context_revision_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_wf_ctx_inst ON workflow_context_revisions (workflow_instance_id);

-- ============================================================
-- Workflow Node Visits
-- ============================================================

CREATE TABLE workflow_node_visits (
    node_visit_id           UUID        NOT NULL PRIMARY KEY,
    workflow_instance_id    UUID        NOT NULL REFERENCES workflow_instances(workflow_instance_id),
    node_id                 UUID        NOT NULL REFERENCES workflow_node_definitions(node_id),
    visit_number            INTEGER     NOT NULL CHECK (visit_number >= 1),
    assignee_principal_id   UUID        NOT NULL REFERENCES principals(principal_id),
    entered_by_transition_id UUID,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- (workflow_instance_id, node_id, visit_number) is unique
    UNIQUE (workflow_instance_id, node_id, visit_number),
    -- Composite unique for FK references
    UNIQUE (node_visit_id, workflow_instance_id)
);

CREATE INDEX idx_wf_visit_inst ON workflow_node_visits (workflow_instance_id);
CREATE INDEX idx_wf_visit_assignee ON workflow_node_visits (assignee_principal_id);
CREATE INDEX idx_wf_visit_node ON workflow_node_visits (node_id);

-- ============================================================
-- Workflow Submissions
-- ============================================================

CREATE TABLE workflow_submissions (
    submission_id           UUID        NOT NULL PRIMARY KEY,
    workflow_instance_id    UUID        NOT NULL REFERENCES workflow_instances(workflow_instance_id),
    source_node_visit_id    UUID        NOT NULL REFERENCES workflow_node_visits(node_visit_id),
    context_revision_id     UUID        NOT NULL REFERENCES workflow_context_revisions(context_revision_id),
    author_principal_id     UUID        NOT NULL REFERENCES principals(principal_id),
    transition_id           UUID        NOT NULL REFERENCES workflow_transition_definitions(transition_id),
    payload                 JSONB       NOT NULL,
    payload_digest          TEXT        NOT NULL CHECK (payload_digest ~ '^[0-9a-f]{64}$'),
    schema_version          TEXT        NOT NULL CHECK (char_length(schema_version) >= 1 AND char_length(schema_version) <= 64),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- At most one committed submission per source_node_visit_id
    UNIQUE (source_node_visit_id),
    -- Composite unique for FK references
    UNIQUE (submission_id, workflow_instance_id)
);

-- Composite FK: source_node_visit_id must belong to same instance
-- This ensures cross-instance references are prevented.
ALTER TABLE workflow_submissions
    ADD CONSTRAINT fk_submission_visit_same_instance
    FOREIGN KEY (source_node_visit_id, workflow_instance_id)
    REFERENCES workflow_node_visits (node_visit_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

-- Composite FK: context_revision_id must belong to same instance
ALTER TABLE workflow_submissions
    ADD CONSTRAINT fk_submission_ctx_same_instance
    FOREIGN KEY (context_revision_id, workflow_instance_id)
    REFERENCES workflow_context_revisions (context_revision_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_wf_sub_inst ON workflow_submissions (workflow_instance_id);
CREATE INDEX idx_wf_sub_visit ON workflow_submissions (source_node_visit_id);
CREATE INDEX idx_wf_sub_author ON workflow_submissions (author_principal_id);
