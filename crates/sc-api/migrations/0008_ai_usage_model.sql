-- The model that served each call, so costs are priced (and can be audited) per model.
ALTER TABLE ai_usage ADD COLUMN model TEXT NOT NULL DEFAULT '';
