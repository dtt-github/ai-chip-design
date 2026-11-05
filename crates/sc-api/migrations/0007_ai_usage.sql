-- What each person's assistant and agent calls cost, for the per-user daily limit.
CREATE TABLE ai_usage (
    id         BIGSERIAL PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users (id),
    project_id UUID REFERENCES projects (id) ON DELETE SET NULL,
    -- 'agent' (a proposal) or 'draft' (spec drafting).
    kind       TEXT NOT NULL,
    usage      JSONB NOT NULL,
    usd        DOUBLE PRECISION NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ai_usage_user_day_idx ON ai_usage (user_id, created_at);
