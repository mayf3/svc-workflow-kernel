-- Migration 0007: Definition Graph Immutability & Additional Invariants
--
-- This migration adds three protections identified by the first audit:
--
-- 1. Definition graph immutability:
--    Protects workflow_node_definitions and workflow_transition_definitions
--    from INSERT, UPDATE, or DELETE when the parent version is not DRAFT.
--
-- 2. Workflow Instance external_url and metadata immutability:
--    The frozen architecture (Section 11) requires these fields to be
--    immutable after creation.
--
-- 3. PROCESSING receipt identity field protection:
--    Once inserted, the identity fields (principal_id, idempotency_key,
--    request_hash, command_type, command_id) must not change.
--
-- Rationale for separate migration:
-- These are new cross-table checks and additional field protections that
-- extend beyond the base triggers in 0006. Keeping them separate avoids
-- bloating 0006 and makes audit trail cleaner.
--
-- Migration order: 0006 must run first to establish base triggers.

-- ============================================================
-- 1. Definition Graph Immutability
-- ============================================================

CREATE OR REPLACE FUNCTION fn_check_definition_graph_immutable()
RETURNS TRIGGER AS $$
DECLARE
    parent_status TEXT;
BEGIN
    -- Determine the definition_version_id based on TG_OP
    IF TG_OP = 'INSERT' OR TG_OP = 'UPDATE' THEN
        parent_status := (
            SELECT v.version_status::TEXT
            FROM workflow_definition_versions v
            WHERE v.definition_version_id = NEW.definition_version_id
        );
    ELSIF TG_OP = 'DELETE' THEN
        parent_status := (
            SELECT v.version_status::TEXT
            FROM workflow_definition_versions v
            WHERE v.definition_version_id = OLD.definition_version_id
        );
    END IF;

    IF parent_status IS NULL THEN
        RAISE EXCEPTION
            'graph_immutable: cannot % on % because parent definition_version_id does not exist',
            TG_OP, TG_TABLE_NAME
            USING ERRCODE = '23000';
    END IF;

    IF parent_status <> 'DRAFT' THEN
        RAISE EXCEPTION
            'graph_immutable: cannot % on % because parent definition version status is "%" (only DRAFT allows modifications)',
            TG_OP, TG_TABLE_NAME, parent_status
            USING ERRCODE = '23000';
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- workflow_node_definitions protection
CREATE TRIGGER trg_node_definitions_graph_immutable
    BEFORE INSERT OR UPDATE OR DELETE ON workflow_node_definitions
    FOR EACH ROW EXECUTE FUNCTION fn_check_definition_graph_immutable();

-- workflow_transition_definitions protection
CREATE TRIGGER trg_transition_definitions_graph_immutable
    BEFORE INSERT OR UPDATE OR DELETE ON workflow_transition_definitions
    FOR EACH ROW EXECUTE FUNCTION fn_check_definition_graph_immutable();

-- ============================================================
-- 2. Workflow Instance external_url and metadata immutability
--
-- Extends fn_check_instance_immutable_fields (created in 0006)
-- to also protect external_url and metadata.
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
    IF OLD.external_url IS DISTINCT FROM NEW.external_url THEN
        RAISE EXCEPTION 'immutable field: workflow_instances.external_url cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.metadata IS DISTINCT FROM NEW.metadata THEN
        RAISE EXCEPTION 'immutable field: workflow_instances.metadata cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- NOTE: The trigger trg_instance_immutable_fields was created in 0006.
-- Since we are REPLACING the function, the existing trigger automatically
-- picks up the new function body. No DROP/CREATE TRIGGER needed.

-- ============================================================
-- 3. PROCESSING Receipt Identity Field Protection
--
-- Freeze identity fields (principal_id, idempotency_key,
-- request_hash, command_type, command_id, created_at) on all
-- command receipts regardless of status.
-- ============================================================

CREATE OR REPLACE FUNCTION fn_check_receipt_identity_immutable()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.command_id IS DISTINCT FROM NEW.command_id THEN
        RAISE EXCEPTION 'immutable field: workflow_command_receipts.command_id cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.principal_id IS DISTINCT FROM NEW.principal_id THEN
        RAISE EXCEPTION 'immutable field: workflow_command_receipts.principal_id cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.idempotency_key IS DISTINCT FROM NEW.idempotency_key THEN
        RAISE EXCEPTION 'immutable field: workflow_command_receipts.idempotency_key cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.command_type IS DISTINCT FROM NEW.command_type THEN
        RAISE EXCEPTION 'immutable field: workflow_command_receipts.command_type cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.request_hash IS DISTINCT FROM NEW.request_hash THEN
        RAISE EXCEPTION 'immutable field: workflow_command_receipts.request_hash cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    IF OLD.created_at IS DISTINCT FROM NEW.created_at THEN
        RAISE EXCEPTION 'immutable field: workflow_command_receipts.created_at cannot be modified'
            USING ERRCODE = '23000';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_receipt_identity_immutable
    BEFORE UPDATE ON workflow_command_receipts
    FOR EACH ROW EXECUTE FUNCTION fn_check_receipt_identity_immutable();
