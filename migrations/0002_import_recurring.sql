-- CSV import (src/import.rs): a fingerprint of each imported row, so importing the same file
-- again adds nothing.
ALTER TABLE transactions ADD COLUMN import_key TEXT;
CREATE UNIQUE INDEX idx_transactions_import_key ON transactions(import_key) WHERE import_key IS NOT NULL;

-- Recurring operations (src/recurring.rs): rent, subscriptions, salary. The server adds each
-- one on its day; the n-th falls on `start_date` plus n periods, so the 31st becomes the last
-- day of a short month without drifting.
CREATE TABLE recurring (
  id BIGSERIAL PRIMARY KEY,
  budget_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  category_id BIGINT NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
  type TEXT NOT NULL CHECK(type IN ('income','expense','savings')),
  amount DOUBLE PRECISION NOT NULL CHECK(amount > 0),
  currency TEXT NOT NULL,
  note TEXT,
  every TEXT NOT NULL CHECK(every IN ('week','month','year')),
  start_date TEXT NOT NULL,
  -- How many have been added; the next is due on start_date + done periods.
  done BIGINT NOT NULL DEFAULT 0,
  next_date TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_recurring_budget ON recurring(budget_id);
CREATE INDEX idx_recurring_next ON recurring(next_date);
ALTER TABLE transactions ADD COLUMN recurring_id BIGINT REFERENCES recurring(id) ON DELETE SET NULL;
