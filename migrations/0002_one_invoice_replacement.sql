-- A voided invoice has at most one replacement. `Invoicing/void_and_reissue` voids, credits and
-- reissues in one batch; when two of them race for the same invoice, the second batch fails on
-- this index (and rolls back its credit note) instead of issuing a second credit note and a
-- second replacement.
CREATE UNIQUE INDEX invoices_one_replacement ON invoices (org_id, supersedes_id)
    WHERE supersedes_id IS NOT NULL;
