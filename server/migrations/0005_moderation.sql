-- Moderation. A suspended account can neither sign in nor share; an account
-- may be let off the weekly link limit, or have its week start over.
ALTER TABLE users ADD COLUMN banned_at INTEGER;
ALTER TABLE users ADD COLUMN unlimited_links INTEGER NOT NULL DEFAULT 0;
-- Links made before this moment no longer count against the week.
ALTER TABLE users ADD COLUMN links_reset_at INTEGER NOT NULL DEFAULT 0;

-- A report from the share page: which clip, why, and an optional line. Nothing
-- about who sent it.
CREATE TABLE reports (
  id TEXT PRIMARY KEY,
  share_id TEXT NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('nsfw', 'violence', 'hate', 'spam', 'other')),
  note TEXT,
  created_at INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'removed', 'dismissed'))
);
CREATE INDEX reports_open ON reports(status, created_at);
CREATE INDEX reports_share ON reports(share_id);
