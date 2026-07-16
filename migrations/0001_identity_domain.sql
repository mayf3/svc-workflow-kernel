-- Migration 0001: Identity & Domain tables
-- Creates enums, principals, domains, and domain_role_bindings

-- ============================================================
-- Enums
-- ============================================================

CREATE TYPE principal_type AS ENUM (
    'HUMAN',
    'AGENT',
    'SERVICE'
);

CREATE TYPE definition_version_status AS ENUM (
    'DRAFT',
    'PUBLISHED',
    'DEPRECATED',
    'REVOKED'
);

CREATE TYPE node_type AS ENUM (
    'DRAFT',
    'NORMAL',
    'TERMINAL'
);

CREATE TYPE assignee_ref_type AS ENUM (
    'WORKFLOW_CREATOR',
    'DOMAIN_OWNER',
    'FIXED_PRINCIPAL'
);

CREATE TYPE transition_effect AS ENUM (
    'ADVANCE',
    'RETURN',
    'TERMINATE'
);

CREATE TYPE receipt_status AS ENUM (
    'PROCESSING',
    'COMPLETED'
);

-- ============================================================
-- Principals
-- ============================================================

CREATE TABLE principals (
    principal_id    UUID        NOT NULL PRIMARY KEY,
    principal_type  principal_type NOT NULL,
    display_name    TEXT        NOT NULL CHECK (char_length(display_name) >= 1 AND char_length(display_name) <= 256),
    email           TEXT        CHECK (email IS NULL OR (char_length(email) >= 3 AND char_length(email) <= 320)),
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,
    metadata        JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_principals_enabled ON principals (enabled) WHERE enabled = TRUE;

-- ============================================================
-- Domains
-- ============================================================

CREATE TABLE domains (
    domain_id       UUID        NOT NULL PRIMARY KEY,
    domain_key      TEXT        NOT NULL,
    display_name    TEXT        NOT NULL CHECK (char_length(display_name) >= 1 AND char_length(display_name) <= 256),
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,
    metadata        JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX idx_domains_key ON domains (domain_key);

-- ============================================================
-- Domain Role Bindings
-- ============================================================

CREATE TABLE domain_role_bindings (
    binding_id      UUID        NOT NULL PRIMARY KEY,
    domain_id       UUID        NOT NULL REFERENCES domains(domain_id),
    principal_id    UUID        NOT NULL REFERENCES principals(principal_id),
    role_key        TEXT        NOT NULL CHECK (char_length(role_key) >= 1 AND char_length(role_key) <= 128),
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at     TIMESTAMPTZ
);

-- Constraint: each principal can have at most one binding per (domain, role)
CREATE UNIQUE INDEX idx_drb_domain_principal_role
    ON domain_role_bindings (domain_id, principal_id, role_key);

-- Constraint: at most one enabled DOMAIN_OWNER per domain
CREATE UNIQUE INDEX idx_drb_single_owner
    ON domain_role_bindings (domain_id, role_key)
    WHERE enabled = TRUE AND role_key = 'DOMAIN_OWNER';

CREATE INDEX idx_drb_principal ON domain_role_bindings (principal_id);
CREATE INDEX idx_drb_domain ON domain_role_bindings (domain_id);
