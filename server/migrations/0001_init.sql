-- Accounts come from Discord; our own id stays stable should someone ever log
-- in another way.
CREATE TABLE users (
  id TEXT PRIMARY KEY,
  discord_id TEXT NOT NULL UNIQUE,
  -- Discord's unique handle, kept in step on every login.
  username TEXT NOT NULL COLLATE NOCASE,
  display_name TEXT NOT NULL,
  avatar TEXT,
  -- Eight characters without look-alikes, shown as XXXX-XXXX.
  friend_code TEXT NOT NULL UNIQUE,
  allow_requests INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
CREATE INDEX users_username ON users(username);

-- Only the hash of a token is stored: a leaked table logs nobody in.
CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  last_used INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);

-- A login in flight. `challenge` is the hash of the verifier the app keeps;
-- after Discord answers, `code` is the one-time code the app trades in.
CREATE TABLE logins (
  state TEXT PRIMARY KEY,
  challenge TEXT NOT NULL,
  code TEXT UNIQUE,
  user_id TEXT,
  created_at INTEGER NOT NULL
);

-- One row per pair, the smaller id first, so a pair can never exist twice.
CREATE TABLE friendships (
  user_a TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  user_b TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  requester TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending', 'accepted')),
  created_at INTEGER NOT NULL,
  PRIMARY KEY (user_a, user_b),
  CHECK (user_a < user_b)
);
CREATE INDEX friendships_b ON friendships(user_b);

CREATE TABLE blocks (
  blocker TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  blocked TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (blocker, blocked)
);
CREATE INDEX blocks_blocked ON blocks(blocked);
