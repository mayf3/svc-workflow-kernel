-- Migration 0009: Add external_reference to workflow_instances
--
-- This column is required by the CreateWorkflowInstance command to
-- store an optional caller-supplied external reference identifier.

ALTER TABLE workflow_instances
    ADD COLUMN IF NOT EXISTS external_reference TEXT
    CHECK (external_reference IS NULL OR char_length(external_reference) <= 512);
