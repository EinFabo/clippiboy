-- How many writes the routes anyone may call made today, per route. Counted
-- before the write; past the day's ceiling they answer 429, so a flood cannot
-- spend the database's daily writes for everybody.
CREATE TABLE anon_budget (
  day TEXT NOT NULL,
  route TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (day, route)
);
