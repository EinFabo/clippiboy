-- What outlives a deleted account, for seven days: a keyed one-way hash of its
-- Discord id (HMAC under the secret DEPARTED_PEPPER), the times of its links
-- that still counted against the week (a JSON array) and a suspension if one
-- stood. A new account for the same Discord id takes them over, so deleting
-- the account and signing in again neither starts the weekly link limit over
-- nor drops a suspension. Nothing else about the account is kept; the hourly
-- cron deletes rows older than seven days.
CREATE TABLE departed (
  hash TEXT PRIMARY KEY,
  link_times TEXT NOT NULL DEFAULT '[]',
  banned_at INTEGER,
  left_at INTEGER NOT NULL
);
CREATE INDEX departed_left ON departed(left_at);
