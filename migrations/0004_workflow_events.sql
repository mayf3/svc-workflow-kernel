-- Migration 0004: Workflow Events
-- Immutable event log for all successful state-changing commands

CREATE TABLE workflow_events (
    event_id                UUID            NOT NULL PRIMARY KEY,
    workflow_instance_id    UUID            NOT NULL REFERENCES workflow_instances(workflow_instance_id),
    event_sequence          INTEGER         NOT NULL CHECK (event_sequence >= 1),
    event_schema_version    TEXT            NOT NULL CHECK (char_length(event_schema_version) >= 1 AND char_length(event_schema_version) <= 64),

    command_id              UUID,
    causation_id            UUID,
    correlation_id          UUID,

    event_type              TEXT            NOT NULL CHECK (char_length(event_type) >= 1 AND char_length(event_type) <= 128),
    transition_effect       transition_effect,

    source_node_visit_id    UUID,
    target_node_visit_id    UUID,

    context_revision_id     UUID,
    submission_id           UUID,

    event_data              JSONB,
    event_data_digest       TEXT            CHECK (event_data_digest IS NULL OR event_data_digest ~ '^[0-9a-f]{64}$'),

    actor_principal_id      UUID            NOT NULL REFERENCES principals(principal_id),
    from_node_id            UUID            REFERENCES workflow_node_definitions(node_id),
    to_node_id              UUID            REFERENCES workflow_node_definitions(node_id),

    old_workflow_state_version  INTEGER     NOT NULL CHECK (old_workflow_state_version >= 0),
    new_workflow_state_version  INTEGER     NOT NULL CHECK (new_workflow_state_version >= 1),

    created_at              TIMESTAMPTZ     NOT NULL DEFAULT now(),

    -- (workflow_instance_id, event_sequence) is unique
    UNIQUE (workflow_instance_id, event_sequence)
);

-- Constraint: new_workflow_state_version = old_workflow_state_version + 1
ALTER TABLE workflow_events
    ADD CONSTRAINT chk_state_version_increment
    CHECK (new_workflow_state_version = old_workflow_state_version + 1);

-- Constraint: event_sequence = new_workflow_state_version
ALTER TABLE workflow_events
    ADD CONSTRAINT chk_event_sequence_equals_state
    CHECK (event_sequence = new_workflow_state_version);

-- Command FK is added in migration 0006 alongside all other deferred constraints
-- to avoid ordering issues with the command_receipts table.

-- Composite FK: source_node_visit_id must belong to same instance
ALTER TABLE workflow_events
    ADD CONSTRAINT fk_event_source_visit_same_instance
    FOREIGN KEY (source_node_visit_id, workflow_instance_id)
    REFERENCES workflow_node_visits (node_visit_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

-- Composite FK: target_node_visit_id must belong to same instance
ALTER TABLE workflow_events
    ADD CONSTRAINT fk_event_target_visit_same_instance
    FOREIGN KEY (target_node_visit_id, workflow_instance_id)
    REFERENCES workflow_node_visits (node_visit_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

-- Composite FK: context_revision_id must belong to same instance
ALTER TABLE workflow_events
    ADD CONSTRAINT fk_event_ctx_same_instance
    FOREIGN KEY (context_revision_id, workflow_instance_id)
    REFERENCES workflow_context_revisions (context_revision_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

-- Composite FK: submission_id must belong to same instance
ALTER TABLE workflow_events
    ADD CONSTRAINT fk_event_submission_same_instance
    FOREIGN KEY (submission_id, workflow_instance_id)
    REFERENCES workflow_submissions (submission_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_wf_event_inst ON workflow_events (workflow_instance_id);
CREATE INDEX idx_wf_event_sequence ON workflow_events (workflow_instance_id, event_sequence);
CREATE INDEX idx_wf_event_command ON workflow_events (command_id);
