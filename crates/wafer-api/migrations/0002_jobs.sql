-- Flow runs (LibreLane RTL-to-GDS for now). Inputs are pinned: a commit and a toolchain digest.
CREATE TABLE jobs (
    id          UUID PRIMARY KEY,
    project_id  UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    created_by  UUID NOT NULL REFERENCES users (id),
    kind        TEXT NOT NULL DEFAULT 'librelane' CHECK (kind IN ('librelane')),
    commit_id   TEXT NOT NULL,
    config_path TEXT NOT NULL,
    pdk         TEXT NOT NULL,
    toolchain   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'queued'
                CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'canceled')),
    exit_code   INTEGER,
    error       TEXT,
    summary     JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);
CREATE INDEX jobs_project_idx ON jobs (project_id, created_at DESC);
CREATE INDEX jobs_queue_idx ON jobs (created_at) WHERE status = 'queued';
