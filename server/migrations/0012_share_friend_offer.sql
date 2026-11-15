-- "Offer friend requests on share links" (Settings → Friends): per link, so a
-- link keeps what was chosen when it went up. Apps before this setting send
-- nothing and keep the button, as before.
ALTER TABLE shares ADD COLUMN offer_friend INTEGER NOT NULL DEFAULT 1;
