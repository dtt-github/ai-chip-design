-- AI agent proposals: changes the agent made on top of `base_commit`, the
-- simulation gate's verdict, and an evidence log. Only a human can merge, and
-- only when the gate passes.
CREATE TABLE agent_proposals (
    id            UUID PRIMARY KEY,
    project_id    UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    created_by    UUID NOT NULL REFERENCES users (id),
    request       TEXT NOT NULL,
    base_commit   TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'running'
                  CHECK (status IN ('running', 'ready', 'gate_failed', 'failed', 'merged', 'discarded')),
    model         TEXT NOT NULL,
    summary       TEXT,
    error         TEXT,
    -- path -> new content, for every file the agent wrote
    files         JSONB NOT NULL DEFAULT '{}'::jsonb,
    gate          JSONB,
    evidence      JSONB NOT NULL DEFAULT '{}'::jsonb,
    evidence_hash TEXT,
    merged_commit TEXT,
    merged_by     UUID REFERENCES users (id),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ,
    merged_at     TIMESTAMPTZ
);
CREATE INDEX agent_proposals_project_idx ON agent_proposals (project_id, created_at DESC);
