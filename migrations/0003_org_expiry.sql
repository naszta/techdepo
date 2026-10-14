-- Automatic expiry of demo orgs (`ORG_EXPIRY_DAYS` on techdepo-services). Set at provisioning
-- when expiry is on, `NULL` otherwise (and for orgs provisioned before it was turned on). When
-- it passes, `Org/expire` deletes the `orgs` row, which cascades to all of the org's data.
ALTER TABLE orgs ADD COLUMN expires_at INTEGER;
