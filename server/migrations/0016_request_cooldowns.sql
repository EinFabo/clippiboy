-- A pair whose friend request was withdrawn or declined rests for ten minutes:
-- neither may ask the other again before `ends_at` (`REQUEST_COOLDOWN_MS` in
-- friendrules.ts). Asking and taking it back in a loop sent the other side a
-- new request every few seconds. One row per pair, the smaller id first, as in
-- friendships; the hourly cron deletes rows whose time is up, and an account's
-- rows go with it.
CREATE TABLE request_cooldowns (
  user_a TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  user_b TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  ends_at INTEGER NOT NULL,
  PRIMARY KEY (user_a, user_b),
  CHECK (user_a < user_b)
) WITHOUT ROWID;
