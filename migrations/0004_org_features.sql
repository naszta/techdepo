-- Per-org features (`domain::OrgFeature`): parts of the app only some orgs get, switched on by
-- the operator (not an admin setting, so not in `settings_json`). Unknown names are ignored.
--   wrangler d1 execute techdepo --local --persist-to ../../.wrangler/state \
--     --command "UPDATE orgs SET features_json = '[\"muhaha\"]' WHERE id = 'org_…'"
ALTER TABLE orgs ADD COLUMN features_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(features_json));  -- [domain::OrgFeature]
