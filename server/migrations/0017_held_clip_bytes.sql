-- A clip held for a report (0015) stays in R2 after its link has gone, so it
-- counts against the bucket's 9 GiB like a live link (`LIVE_BYTES` in
-- shares.ts): its size is noted when it is held (`HOLD_CLIP`), read from the
-- link's row while that is still there. Clips held before this count as 0
-- until the sweep lets them go.
--
-- 0015 says the report fingerprint takes an IPv6 address per /64; it takes
-- the /48 (`addressKey` in limits.ts), as every limit per address does.
ALTER TABLE held_clips ADD COLUMN bytes INTEGER NOT NULL DEFAULT 0;
