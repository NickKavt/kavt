-- A monthly spending limit on a root expense category, in one currency. NULL means none.
ALTER TABLE categories ADD COLUMN IF NOT EXISTS limit_amount DOUBLE PRECISION;
ALTER TABLE categories ADD COLUMN IF NOT EXISTS limit_currency TEXT;
ALTER TABLE categories DROP CONSTRAINT IF EXISTS categories_limit_check;
ALTER TABLE categories ADD CONSTRAINT categories_limit_check
  CHECK ((limit_amount IS NULL AND limit_currency IS NULL) OR (limit_amount > 0 AND limit_currency IS NOT NULL));
