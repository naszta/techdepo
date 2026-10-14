-- App-wide settings, not per org: the operator's (the Muhaha page, `Muhaha` service). One row
-- per key; the only key is `models` (domain::ModelSettings). Without a row the deployment's
-- defaults apply (`LLM_BACKEND`, `AI_*`), so an empty table changes nothing.
CREATE TABLE app_settings (
    key        TEXT    NOT NULL PRIMARY KEY CHECK (key IN ('models')),
    value_json TEXT    NOT NULL CHECK (json_valid(value_json)),
    updated_at INTEGER NOT NULL,
    updated_by TEXT                                -- user ID of the operator
) STRICT;
