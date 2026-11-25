-- A reported clip is not deleted while a report on it is open. When its link
-- goes — deleted by its owner, run out, its account deleted or suspended — the
-- clip and its poster stay in R2 under their keys, and this row says so. The
-- link is gone with its share row, so nobody reaches them. The hourly sweep
-- deletes them once no report on the clip is open, after 30 days at the most
-- (`heldClipGoes` in shares.ts). No foreign key: the share row may go first.
CREATE TABLE held_clips (
  share_id TEXT PRIMARY KEY,
  held_at INTEGER NOT NULL
);

-- Who reported, as a one-way fingerprint of the address (an IPv6 address per
-- /64) made for that clip alone: each address counts once against a clip's
-- cap of open reports. Goes with the report. Reports from before have none.
ALTER TABLE reports ADD COLUMN reporter TEXT;
