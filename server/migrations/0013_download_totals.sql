-- Downloads through clippiboy.com/get/<ref>, added up for good. `downloads`
-- keeps a count per day and is swept after 400 days; this keeps the sum, so the
-- number on the page never shrinks. Seeded from the days still there.
CREATE TABLE download_totals (
  ref TEXT PRIMARY KEY,
  count INTEGER NOT NULL DEFAULT 0
);
INSERT INTO download_totals (ref, count) SELECT ref, SUM(count) FROM downloads GROUP BY ref;
