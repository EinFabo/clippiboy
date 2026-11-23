-- The hourly sweep deletes installs not seen for 400 days (telemetry.ts,
-- `sweepUsage`). Without an index each run read the whole table.
CREATE INDEX installs_last_day ON installs(last_day);
