-- Each project declares its licence (SPDX identifier); IP imports are checked against it.
ALTER TABLE projects ADD COLUMN licence TEXT NOT NULL DEFAULT 'Apache-2.0';

-- IP registry: a block is a name owned by one project; a version pins that
-- project's design files at one commit. Versions are immutable.
CREATE TABLE ip_blocks (
    name        TEXT PRIMARY KEY CHECK (name ~ '^[a-z][a-z0-9_]{1,62}$'),
    project_id  UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    description TEXT NOT NULL DEFAULT '',
    created_by  UUID NOT NULL REFERENCES users (id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE ip_versions (
    block        TEXT NOT NULL REFERENCES ip_blocks (name) ON DELETE CASCADE,
    version      TEXT NOT NULL,
    major        INT NOT NULL,
    minor        INT NOT NULL,
    patch        INT NOT NULL,
    commit_id    TEXT NOT NULL,
    top          TEXT NOT NULL,
    -- SPDX expression combined from the files' SPDX-License-Identifier headers.
    licence      TEXT NOT NULL,
    -- Published files: path in the source project -> sha256.
    files        JSONB NOT NULL,
    ports        JSONB NOT NULL,
    params       JSONB NOT NULL,
    -- The source project's simulation gate at `commit_id` (must have passed).
    gate         JSONB NOT NULL,
    published_by UUID NOT NULL REFERENCES users (id),
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (block, version),
    UNIQUE (block, major, minor, patch)
);
