-- More currencies (src/rates.rs): any ISO code besides the four symbols, and currency-api
-- (fawaz) as a source of rates that lists them all.
ALTER TABLE budget_settings DROP CONSTRAINT budget_settings_rate_source_check;
ALTER TABLE budget_settings ADD CONSTRAINT budget_settings_rate_source_check CHECK (rate_source IN ('tcmb', 'cbr', 'ecb', 'fawaz', 'off'));
-- Each person's own choice: which currencies the app offers them (NULL: those the budget uses),
-- and whether amounts in other currencies are also shown converted into the main one.
ALTER TABLE users ADD COLUMN currencies TEXT;
ALTER TABLE users ADD COLUMN show_converted BOOLEAN NOT NULL DEFAULT TRUE;
