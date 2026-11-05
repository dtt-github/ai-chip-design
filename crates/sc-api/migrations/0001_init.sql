-- Users are created on first authenticated request; identity lives in the OIDC provider.
CREATE TABLE users (
    id           UUID PRIMARY KEY,
    oidc_issuer  TEXT NOT NULL,
    oidc_subject TEXT NOT NULL,
    email        TEXT NOT NULL,
    display_name TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (oidc_issuer, oidc_subject)
);
CREATE INDEX users_email_idx ON users (lower(email));

-- A project is a bare Git repository (keyed by id) plus this metadata row.
CREATE TABLE projects (
    id          UUID PRIMARY KEY,
    owner_id    UUID NOT NULL REFERENCES users (id),
    slug        TEXT NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    name        TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '',
    pdk         TEXT NOT NULL DEFAULT 'ihp-sg13g2',
    visibility  TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'public')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (owner_id, slug)
);

CREATE TABLE project_members (
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id),
    role       TEXT NOT NULL CHECK (role IN ('owner', 'editor', 'viewer')),
    added_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX project_members_user_idx ON project_members (user_id);
