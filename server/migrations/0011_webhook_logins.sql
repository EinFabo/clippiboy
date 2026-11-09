-- "Post clips to a Discord channel": from the app's start to Discord's answer.
-- Holds the app's one-time key the callback seals the webhook with — never the
-- webhook itself. Spent by the callback, swept after ten minutes.
CREATE TABLE webhook_logins (
  state TEXT PRIMARY KEY,
  key TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
