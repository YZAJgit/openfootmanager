-- Youth academy (M7).
--
-- A club's EPPP-style academy category (1-4, NULL reads as Category Three) and a player's
-- academy record as JSON: when he joined, his scholarship decision deadline and the pre-21
-- training spells the home-grown rules count. NULL for every row written before the columns,
-- which reads as no academy membership and a Category Three academy.
ALTER TABLE teams ADD COLUMN academy_tier INTEGER;
ALTER TABLE players ADD COLUMN academy_json TEXT;
