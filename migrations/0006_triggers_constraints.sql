-- Migration 0006: Triggers, functions, and remaining constraints
-- Immutable records, size limits, lifecycle enforcement

-- ============================================================
-- Add event->command FK (deferred, now that command_receipts exists)
-- ============================================================
ALTER TABLE workflow_events
    ADD CONSTRAINT fk_event_command
    FOREIGN KEY (command_id)
    REFERENCES workflow_command_receipts(command_id)
    DEFERRABLE INITIALLY DEFERRED;

-- ============================================================
-- Add WorkflowInstance circular composite FKs (deferred)
-- These reference context_revisions and node_visits which
-- themselves reference the instance — hence DEFERRABLE.
-- ============================================================

-- Instance -> current Context Revision (composite FK)
ALTER TABLE workflow_instances
    ADD CONSTRAINT fk_instance_current_ctx
    FOREIGN KEY (current_context_revision_id, workflow_instance_id)
    REFERENCES workflow_context_revisions (context_revision_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

-- Instance -> current Node Visit (composite FK)
ALTER TABLE workflow_instances
    ADD CONSTRAINT fk_instance_current_visit
    FOREIGN KEY (current_node_visit_id, workflow_instance_id)
    REFERENCES workflow_node_visits (node_visit_id, workflow_instance_id)
    DEFERRABLE INITIALLY DEFERRED;

-- ============================================================
-- Constraint: a successful command at most one event
-- command_id is unique in workflow_events for non-null commands
-- ============================================================
CREATE UNIQUE INDEX idx_wf_event_unique_command
    ON workflow_events (command_id)
    WHERE command_id IS NOT NULL;

-- ============================================================
-- Function: prevent UPDATE and DELETE on immutable tables
-- ============================================================

CREATE OR REPLACE FUNCTION fn_prevent_modification()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'immutable record: % table % does not allow %',
        TG_TABLE_NAME,
        TG_TABLE_SCHEMA,
        TG_OP
        USING ERRCODE = '23000';  -- integrity_constraint_violation
END;
$$ LANGUAGE plpgsql;

-- Apply to immutable fact tables
CREATE TRIGGER trg_context_revisions_immutable
    BEFORE UPDATE OR DELETE ON workflow_context_revisions
    FOR EACH ROW EXECUTE FUNCTION fn_prevent_modification();

CREATE TRIGGER trg_node_visits_immutable
    BEFORE UPDATE OR DELETE ON workflow_node_visits
    FOR EACH ROW EXECUTE FUNCTION fn_prevent_modification();

CREATE TRIGGER trg_submissions_immutable
    BEFORE UPDATE OR DELETE ON workflow_submissions
    FOR EACH ROW EXECUTE FUNCTION fn_prevent_modification();

CREATE TRIGGER trg_events_immutable
    BEFORE UPDATE OR DELETE ON workflow_events
    FOR EACH ROW EXECUTE FUNCTION fn_prevent_modification();

-- ============================================================
-- Function: prevent UPDATE on COMPLETED command receipts
-- Also prevent their deletion
-- ============================================================

CREATE OR REPLACE FUNCTION fn_prevent_completed_receipt_modification()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.receipt_status = 'COMPLETED' THEN
        RAISE EXCEPTION 'immutable record: COMPLETED receipt % cannot be modified', OLD.command_id
            USING ERRCODE = '23000';
    END IF;
    IF TG_OP = 'DELETE' AND OLD.receipt_status = 'COMPLETED' THEN
        RAISE EXCEPTION 'immutable record: COMPLETED receipt % cannot be deleted', OLD.command_id
            USING ERRCODE = '23000';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_command_receipts_completed_immutable
    BEFORE UPDATE OR DELETE ON workflow_command_receipts
    FOR EACH ROW EXECUTE FUNCTION fn_prevent_completed_receipt_modification();

-- ============================================================
-- Constraint: Receipt status lifecycle (PROCESSING -> COMPLETED only)
-- ============================================================

CREATE OR REPLACE FUNCTION fn_check_receipt_status_transition()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.receipt_status = 'PROCESSING' AND NEW.receipt_status = 'COMPLETED' THEN
        RETURN NEW;
    END IF;
    IF OLD.receipt_status = NEW.receipt_status THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'invalid receipt status transition: % -> %', OLD.receipt_status, NEW.receipt_status
        USING ERRCODE = '23000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_command_receipts_status_check
    BEFORE UPDATE OF receipt_status ON workflow_command_receipts
    FOR EACH ROW EXECUTE FUNCTION fn_check_receipt_status_transition();

-- ============================================================
-- Function: prevent modification of immutable Instance fields
-- ============================================================

CREATE OR REPLACE FUNCTION fn_check_instance_immutable_fields()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.domain_id IS DISTINCT FROM NEW.domain_id THEN
        RAISE EXCEPTION 'immutable field: workflow_instances.domain_id cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.definition_version_id IS DISTINCT FROM NEW.definition_version_id THEN
        RAISE EXCEPTION 'immutable field: workflow_instances.definition_version_id cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.created_by_principal_id IS DISTINCT FROM NEW.created_by_principal_id THEN
        RAISE EXCEPTION 'immutable field: workflow_instances.created_by_principal_id cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.created_at IS DISTINCT FROM NEW.created_at THEN
        RAISE EXCEPTION 'immutable field: workflow_instances.created_at cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_instance_immutable_fields
    BEFORE UPDATE ON workflow_instances
    FOR EACH ROW EXECUTE FUNCTION fn_check_instance_immutable_fields();

-- ============================================================
-- Function: prevent modification of published/immutable definition versions
-- ============================================================

CREATE OR REPLACE FUNCTION fn_check_definition_version_immutable()
RETURNS TRIGGER AS $$
BEGIN
    -- Allow status changes, but not field changes for non-DRAFT versions
    IF OLD.version_status = 'DRAFT' THEN
        -- Draft versions allow all changes
        RETURN NEW;
    END IF;

    -- For PUBLISHED, DEPRECATED, REVOKED: only allow status field changes
    IF OLD.definition_digest IS DISTINCT FROM NEW.definition_digest
        OR OLD.json_schema_dialect IS DISTINCT FROM NEW.json_schema_dialect
        OR OLD.validator_version IS DISTINCT FROM NEW.validator_version
        OR OLD.context_schema IS DISTINCT FROM NEW.context_schema
        OR OLD.submission_schema IS DISTINCT FROM NEW.submission_schema
        OR OLD.metadata IS DISTINCT FROM NEW.metadata
    THEN
        RAISE EXCEPTION 'immutable field: definition_version % (status=%) business fields cannot be modified',
            OLD.definition_version_id, OLD.version_status
            USING ERRCODE = '23000';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_definition_version_immutable
    BEFORE UPDATE ON workflow_definition_versions
    FOR EACH ROW EXECUTE FUNCTION fn_check_definition_version_immutable();

-- ============================================================
-- Function: reject illegal definition version status transitions
-- Allowed transitions:
--   DRAFT -> PUBLISHED (normal publish)
--   PUBLISHED -> DEPRECATED
--   PUBLISHED -> REVOKED
--   DEPRECATED -> REVOKED
-- Forbidden:
--   PUBLISHED -> DRAFT
--   DEPRECATED -> PUBLISHED
--   REVOKED -> anything
--   DEPRECATED -> DRAFT
-- ============================================================

CREATE OR REPLACE FUNCTION fn_check_definition_version_status_transition()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.version_status = NEW.version_status THEN
        RETURN NEW;
    END IF;

    -- Define allowed transitions
    IF OLD.version_status = 'DRAFT' AND NEW.version_status = 'PUBLISHED' THEN
        RETURN NEW;
    END IF;

    IF OLD.version_status = 'PUBLISHED' AND NEW.version_status IN ('DEPRECATED', 'REVOKED') THEN
        RETURN NEW;
    END IF;

    IF OLD.version_status = 'DEPRECATED' AND NEW.version_status = 'REVOKED' THEN
        RETURN NEW;
    END IF;

    RAISE EXCEPTION 'illegal definition version status transition: % -> %',
        OLD.version_status, NEW.version_status
        USING ERRCODE = '23000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_definition_version_status_transition
    BEFORE UPDATE OF version_status ON workflow_definition_versions
    FOR EACH ROW EXECUTE FUNCTION fn_check_definition_version_status_transition();

-- ============================================================
-- Size limit checks (using pg_column_size for JSONB, octet_length for text)
-- Note: pg_column_size returns the storage size of the jsonb value
-- in its binary JSONB representation (including type overhead).
-- These are defensive hard limits; finer-grained validation
-- should also be performed in the Rust service layer.
-- ============================================================

-- Context payload ≤ 1 MiB
ALTER TABLE workflow_context_revisions
    ADD CONSTRAINT chk_ctx_payload_size
    CHECK (pg_column_size(payload) <= 1048576);

-- Submission payload ≤ 1 MiB
ALTER TABLE workflow_submissions
    ADD CONSTRAINT chk_submission_payload_size
    CHECK (pg_column_size(payload) <= 1048576);

-- metadata ≤ 64 KiB
ALTER TABLE workflow_instances
    ADD CONSTRAINT chk_instance_metadata_size
    CHECK (metadata IS NULL OR pg_column_size(metadata) <= 65536);

ALTER TABLE workflow_definitions
    ADD CONSTRAINT chk_def_metadata_size
    CHECK (metadata IS NULL OR pg_column_size(metadata) <= 65536);

ALTER TABLE workflow_definition_versions
    ADD CONSTRAINT chk_def_ver_metadata_size
    CHECK (metadata IS NULL OR pg_column_size(metadata) <= 65536);

-- CommandReceipt responseBody ≤ 1 MiB
ALTER TABLE workflow_command_receipts
    ADD CONSTRAINT chk_receipt_response_size
    CHECK (response_body IS NULL OR pg_column_size(response_body) <= 1048576);

-- eventData ≤ 256 KiB
ALTER TABLE workflow_events
    ADD CONSTRAINT chk_event_data_size
    CHECK (event_data IS NULL OR pg_column_size(event_data) <= 262144);

-- ============================================================
-- Function: receipt status can only go PROCESSING -> COMPLETED
-- Prevent PROCESSING -> PROCESSING (no-op) and any other transition
-- ============================================================

-- The trigger above handles this already via fn_check_receipt_status_transition

-- ============================================================
-- Receipt status: COMPLETED receipts set completed_at on transition
-- ============================================================

CREATE OR REPLACE FUNCTION fn_set_completed_at()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.receipt_status = 'COMPLETED' AND OLD.receipt_status = 'PROCESSING' THEN
        NEW.completed_at = now();
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_receipt_set_completed_at
    BEFORE UPDATE OF receipt_status ON workflow_command_receipts
    FOR EACH ROW EXECUTE FUNCTION fn_set_completed_at();

-- ============================================================
-- Receipt status: enforce that response fields are set on COMPLETED
-- ============================================================

CREATE OR REPLACE FUNCTION fn_check_completed_receipt_fields()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.receipt_status = 'COMPLETED' THEN
        IF NEW.response_status IS NULL THEN
            RAISE EXCEPTION 'COMPLETED receipt must have response_status set'
                USING ERRCODE = '23000';
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_receipt_check_completed_fields
    BEFORE UPDATE OF receipt_status ON workflow_command_receipts
    FOR EACH ROW EXECUTE FUNCTION fn_check_completed_receipt_fields();
