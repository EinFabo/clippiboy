-- The hourly sweep deletes installs not seen for 400 days (telemetry.ts,
-- `sweepUsage`). Without an index each run read the whole table.
CREATE INDEX IF NOT EXISTS installs_last_day ON installs(last_day);

-- The hourly cron deletes sessions unused for 180 days.
CREATE INDEX IF NOT EXISTS sessions_last_used ON sessions(last_used);

-- An upload that failed is kept as a deleted share with `failed` 1. A few a
-- week are free, past that each counts against the week like a link
-- (`weekTimes` in shares.ts). Rows from before keep 0: until now a failed
-- upload left no row at all.
ALTER TABLE shares ADD COLUMN failed INTEGER NOT NULL DEFAULT 0;
