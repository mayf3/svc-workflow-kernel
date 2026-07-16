-- Migration 0008: Fix Graph Definition Version ID Escape Path
--
-- This migration closes the narrow escape path identified in the
-- POSTGRES_STORAGE_FOUNDATION_REAUDIT_REPORT (Section 13):
--
-- The trigger fn_check_definition_graph_immutable() in migration 0007
-- only checks NEW.definition_version_id for UPDATE operations. This
-- allows a user to change definition_version_id on a node or transition
-- record from a PUBLISHED/DEPRECATED/REVOKED version to a DRAFT version
-- in the same UPDATE statement, effectively removing the record from
-- the published graph.
--
-- Fix: When TG_OP = 'UPDATE', also verify that OLD.definition_version_id
-- is DRAFT. If OLD parent is not DRAFT, reject the UPDATE regardless of
-- what NEW.definition_version_id points to.
--
-- Also checks: OLD.definition_version_id vs NEW.definition_version_id
-- differ AND OLD parent is not DRAFT → reject.

CREATE OR REPLACE FUNCTION fn_check_definition_graph_immutable()
RETURNS TRIGGER AS $$
DECLARE
    parent_status TEXT;
    old_parent_status TEXT;
BEGIN
    -- For UPDATE, verify both OLD and NEW parent versions
    IF TG_OP = 'UPDATE' THEN
        -- Check NEW parent status
        parent_status := (
            SELECT v.version_status::TEXT
            FROM workflow_definition_versions v
            WHERE v.definition_version_id = NEW.definition_version_id
        );

        IF parent_status IS NULL THEN
            RAISE EXCEPTION
                'graph_immutable: cannot % on % because new parent definition_version_id does not exist',
                TG_OP, TG_TABLE_NAME
                USING ERRCODE = '23000';
        END IF;

        IF parent_status <> 'DRAFT' THEN
            RAISE EXCEPTION
                'graph_immutable: cannot % on % because new parent definition version status is "%" (only DRAFT allows modifications)',
                TG_OP, TG_TABLE_NAME, parent_status
                USING ERRCODE = '23000';
        END IF;

        -- Also check OLD parent: if OLD is not DRAFT and definition_version_id changed, reject
        IF OLD.definition_version_id IS DISTINCT FROM NEW.definition_version_id THEN
            old_parent_status := (
                SELECT v.version_status::TEXT
                FROM workflow_definition_versions v
                WHERE v.definition_version_id = OLD.definition_version_id
            );

            IF old_parent_status IS NOT NULL AND old_parent_status <> 'DRAFT' THEN
                RAISE EXCEPTION
                    'graph_immutable: cannot change definition_version_id on % because old parent version status is "%" (non-DRAFT records cannot be moved)',
                    TG_TABLE_NAME, old_parent_status
                    USING ERRCODE = '23000';
            END IF;
        END IF;
    END IF;

    -- For INSERT, check NEW parent status only
    IF TG_OP = 'INSERT' THEN
        parent_status := (
            SELECT v.version_status::TEXT
            FROM workflow_definition_versions v
            WHERE v.definition_version_id = NEW.definition_version_id
        );

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
    END IF;

    -- For DELETE, check OLD parent status only
    IF TG_OP = 'DELETE' THEN
        parent_status := (
            SELECT v.version_status::TEXT
            FROM workflow_definition_versions v
            WHERE v.definition_version_id = OLD.definition_version_id
        );

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
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Also add the publication lifecycle tracking columns
-- These track WHO performed each lifecycle action
ALTER TABLE workflow_definition_versions
    ADD COLUMN IF NOT EXISTS published_by_principal_id UUID REFERENCES principals(principal_id),
    ADD COLUMN IF NOT EXISTS deprecated_by_principal_id UUID REFERENCES principals(principal_id),
    ADD COLUMN IF NOT EXISTS revoked_by_principal_id UUID REFERENCES principals(principal_id);
