-- The suspension the hourly cron last carried out, by its `banned_at`: each one
-- is carried out once instead of every hour for as long as it stands.
ALTER TABLE users ADD COLUMN suspension_applied_at INTEGER;
