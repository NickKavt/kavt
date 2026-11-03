-- Settings of a budget (src/rates.rs): the currency other currencies are converted into and
-- whose rates are used. 'off' keeps every operation in its own currency only.
CREATE TABLE budget_settings (
  budget_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  main_currency TEXT NOT NULL DEFAULT '₺',
  rate_source TEXT NOT NULL DEFAULT 'tcmb' CHECK (rate_source IN ('tcmb', 'cbr', 'ecb', 'off'))
);

-- Rates a source gave for a day, so a day is fetched once. `in_try`: lira for one unit of each
-- currency; `published`: the day the source dated them (the last working day for weekends).
CREATE TABLE daily_rates (
  source TEXT NOT NULL,
  day TEXT NOT NULL,
  published TEXT NOT NULL,
  in_try JSONB NOT NULL,
  fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (source, day)
);

-- The rate of an operation on its own day: units of `rate_to` (the budget's main currency then)
-- for one unit of the operation's currency. `rate_source` NULL means not looked up yet;
-- 'same' is an operation already in the main currency, 'none' one the source has no rate for.
ALTER TABLE transactions ADD COLUMN rate DOUBLE PRECISION;
ALTER TABLE transactions ADD COLUMN rate_to TEXT;
ALTER TABLE transactions ADD COLUMN rate_day TEXT;
ALTER TABLE transactions ADD COLUMN rate_source TEXT;
CREATE INDEX idx_transactions_rate_pending ON transactions(id) WHERE rate_source IS NULL;
