-- A share is pending while its clip is still on the way up. An upload that
-- broke off (the app closed, the line dropped) can leave such a row behind
-- without its catch ever running; after half an hour it no longer counts
-- against the week or the bucket, and the hourly sweep removes it.
ALTER TABLE shares ADD COLUMN pending INTEGER NOT NULL DEFAULT 0;
