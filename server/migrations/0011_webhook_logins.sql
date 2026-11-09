-- "Post clips to a Discord channel": from the app's start to Discord's answer.
-- Holds the app's one-time key the callback seals the webhook with — never the
-- webhook itself. `started` makes the start page work once; the callback
-- spends the row, the cron sweeps it after ten minutes.
CREATE TABLE webhook_logins (
  state TEXT PRIMARY KEY,
  key TEXT NOT NULL,
  started INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
