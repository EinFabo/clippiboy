-- Share links: a clip in R2 under its id, for five days. The row outlives the
-- file by a while, so a deleted link still counts against its owner's week.
CREATE TABLE shares (
  id TEXT PRIMARY KEY,
  owner TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  bytes INTEGER NOT NULL,
  title TEXT NOT NULL,
  game TEXT,
  width INTEGER NOT NULL DEFAULT 0,
  height INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  deleted_at INTEGER
);
CREATE INDEX shares_owner ON shares(owner, created_at);
CREATE INDEX shares_live ON shares(deleted_at, expires_at);
