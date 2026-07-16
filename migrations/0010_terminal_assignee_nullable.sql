-- Migration 0010: PR5 administrative recovery prerequisites.
--
-- The approved semantic migration was called "0009" during planning, but
-- 0009_add_instance_external_reference.sql already exists in the immutable
-- migration history.  Therefore this migration uses the next sequence.

-- Terminal nodes and visits do not have an assignee. Existing published
-- terminal rows are grandfathered: NOT VALID keeps existing rows untouched,
-- while PostgreSQL still enforces the constraint for every new/updated row.
ALTER TABLE workflow_node_definitions
    ALTER COLUMN assignee_ref_type DROP NOT NULL;

ALTER TABLE workflow_node_definitions
    DROP CONSTRAINT chk_fixed_principal;

ALTER TABLE workflow_node_definitions
    ADD CONSTRAINT chk_node_assignee_shape
    CHECK (
        (node_type = 'TERMINAL'
            AND assignee_ref_type IS NULL
            AND fixed_principal_id IS NULL)
        OR
        (node_type <> 'TERMINAL'
            AND assignee_ref_type IS NOT NULL
            AND (
                (assignee_ref_type = 'FIXED_PRINCIPAL'
                    AND fixed_principal_id IS NOT NULL)
                OR
                (assignee_ref_type IN ('WORKFLOW_CREATOR', 'DOMAIN_OWNER')
                    AND fixed_principal_id IS NULL)
            ))
    ) NOT VALID;

ALTER TABLE workflow_node_visits
    ALTER COLUMN assignee_principal_id DROP NOT NULL;

CREATE OR REPLACE FUNCTION fn_check_node_visit_assignee()
RETURNS TRIGGER AS $$
DECLARE
    target_node_type node_type;
    target_definition_version_id UUID;
    instance_definition_version_id UUID;
BEGIN
    SELECT node_type, definition_version_id
      INTO target_node_type, target_definition_version_id
      FROM workflow_node_definitions
     WHERE node_id = NEW.node_id;

    SELECT definition_version_id INTO instance_definition_version_id
      FROM workflow_instances
     WHERE workflow_instance_id = NEW.workflow_instance_id;

    IF target_node_type IS NULL THEN
        RAISE EXCEPTION 'node visit references a missing node definition'
            USING ERRCODE = '23503';
    END IF;
    IF instance_definition_version_id IS NULL THEN
        RAISE EXCEPTION 'node visit references a missing workflow instance'
            USING ERRCODE = '23503';
    END IF;
    IF target_definition_version_id <> instance_definition_version_id THEN
        RAISE EXCEPTION 'node visit and workflow instance definition versions differ'
            USING ERRCODE = '23514';
    END IF;
    IF target_node_type = 'TERMINAL' AND NEW.assignee_principal_id IS NOT NULL THEN
        RAISE EXCEPTION 'terminal node visit must not have an assignee'
            USING ERRCODE = '23514';
    END IF;
    IF target_node_type <> 'TERMINAL' AND NEW.assignee_principal_id IS NULL THEN
        RAISE EXCEPTION 'non-terminal node visit must have an assignee'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_node_visit_assignee
    BEFORE INSERT OR UPDATE OF workflow_instance_id, node_id, assignee_principal_id
    ON workflow_node_visits
    FOR EACH ROW EXECUTE FUNCTION fn_check_node_visit_assignee();
