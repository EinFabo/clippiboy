-- What the page shows besides the clip: its tags, and — only if the uploader
-- allows it — their Discord name and picture, read from `users` when the page
-- is opened, so a renamed account shows its current name.
ALTER TABLE shares ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE shares ADD COLUMN show_name INTEGER NOT NULL DEFAULT 0;
