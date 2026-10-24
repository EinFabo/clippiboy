-- Discord hands a released name to someone else. Until the old owner signed in
-- again both rows held it, and a request by name could reach the wrong one.
-- Names held twice are cleared; each comes back with its owner's next sign-in.
UPDATE users SET username = '' WHERE username IN (
  SELECT username FROM users WHERE username != '' GROUP BY username HAVING COUNT(*) > 1
);
DROP INDEX users_username;
CREATE UNIQUE INDEX users_username ON users(username) WHERE username != '';
