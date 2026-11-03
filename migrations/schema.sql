CREATE SEQUENCE IF NOT EXISTS users_id_seq;
CREATE SEQUENCE IF NOT EXISTS categories_id_seq;
CREATE SEQUENCE IF NOT EXISTS transactions_id_seq;
CREATE SEQUENCE IF NOT EXISTS jobs_id_seq;

CREATE TABLE IF NOT EXISTS users (
  id BIGINT PRIMARY KEY DEFAULT nextval('users_id_seq'),
  telegram_id TEXT NOT NULL UNIQUE,
  username TEXT,
  first_name TEXT,
  last_name TEXT,
  language_code TEXT,
  role TEXT NOT NULL DEFAULT 'user' CHECK(role IN ('user','admin','owner')),
  created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP::text),
  budget_owner_id BIGINT
);
ALTER SEQUENCE users_id_seq OWNED BY users.id;
ALTER TABLE users ALTER COLUMN id SET DEFAULT nextval('users_id_seq');

CREATE TABLE IF NOT EXISTS categories (
  id BIGINT PRIMARY KEY DEFAULT nextval('categories_id_seq'),
  name TEXT NOT NULL,
  emoji TEXT NOT NULL DEFAULT '📦',
  color TEXT NOT NULL DEFAULT '#3b82f6',
  type TEXT NOT NULL DEFAULT 'expense' CHECK(type IN ('income','expense','savings')),
  parent_id BIGINT REFERENCES categories(id) ON DELETE CASCADE,
  created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP::text),
  target_amount DOUBLE PRECISION
);
ALTER SEQUENCE categories_id_seq OWNED BY categories.id;
ALTER TABLE categories ALTER COLUMN id SET DEFAULT nextval('categories_id_seq');

CREATE TABLE IF NOT EXISTS transactions (
  id BIGINT PRIMARY KEY DEFAULT nextval('transactions_id_seq'),
  user_id BIGINT REFERENCES users(id) ON DELETE CASCADE,
  category_id BIGINT REFERENCES categories(id) ON DELETE CASCADE,
  type TEXT NOT NULL CHECK(type IN ('income','expense','savings')),
  amount DOUBLE PRECISION NOT NULL CHECK(amount >= 0),
  currency TEXT NOT NULL DEFAULT 'TRY',
  note TEXT,
  is_debt BIGINT NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP::text),
  transaction_date TEXT NOT NULL DEFAULT (CURRENT_DATE::text)
);
ALTER SEQUENCE transactions_id_seq OWNED BY transactions.id;
ALTER TABLE transactions ALTER COLUMN id SET DEFAULT nextval('transactions_id_seq');


CREATE TABLE IF NOT EXISTS jobs (
  id BIGINT PRIMARY KEY DEFAULT nextval('jobs_id_seq'),
  kind TEXT NOT NULL,
  payload TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','done','failed')),
  attempts BIGINT NOT NULL DEFAULT 0,
  max_attempts BIGINT NOT NULL DEFAULT 5,
  run_at BIGINT NOT NULL DEFAULT EXTRACT(EPOCH FROM NOW())::BIGINT,
  locked_at BIGINT,
  last_error TEXT,
  created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP::text),
  updated_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP::text)
);
ALTER SEQUENCE jobs_id_seq OWNED BY jobs.id;
ALTER TABLE jobs ALTER COLUMN id SET DEFAULT nextval('jobs_id_seq');

CREATE TABLE IF NOT EXISTS sessions (
  token TEXT PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at BIGINT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP::text),
  ip_address TEXT,
  user_agent TEXT,
  last_used_at TEXT,
  token_suffix TEXT
);

CREATE INDEX IF NOT EXISTS idx_users_telegram_id ON users(telegram_id);
-- One owner per installation, even when two people sign in at once on a fresh install.
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_single_owner ON users(role) WHERE role = 'owner';
CREATE INDEX IF NOT EXISTS idx_categories_parent_id ON categories(parent_id);
CREATE INDEX IF NOT EXISTS idx_transactions_user_id ON transactions(user_id);
CREATE INDEX IF NOT EXISTS idx_transactions_category_id ON transactions(category_id);
CREATE INDEX IF NOT EXISTS idx_transactions_created_at ON transactions(created_at);
CREATE INDEX IF NOT EXISTS idx_transactions_transaction_date ON transactions(transaction_date);
CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_token_suffix ON sessions(token_suffix);
CREATE INDEX IF NOT EXISTS idx_transactions_user_date_type_currency ON transactions(user_id, transaction_date, type, currency);
CREATE INDEX IF NOT EXISTS idx_transactions_category_type_user_date ON transactions(category_id, type, user_id, transaction_date);
CREATE INDEX IF NOT EXISTS idx_transactions_type_date_currency ON transactions(type, transaction_date, currency);
CREATE INDEX IF NOT EXISTS idx_jobs_status_run_at ON jobs(status, run_at, id);
CREATE INDEX IF NOT EXISTS idx_jobs_kind_status ON jobs(kind, status);

SELECT setval('users_id_seq', COALESCE((SELECT MAX(id) FROM users), 0) + 1, false);
SELECT setval('categories_id_seq', COALESCE((SELECT MAX(id) FROM categories), 0) + 1, false);
SELECT setval('transactions_id_seq', COALESCE((SELECT MAX(id) FROM transactions), 0) + 1, false);
SELECT setval('jobs_id_seq', COALESCE((SELECT MAX(id) FROM jobs), 0) + 1, false);

-- Month-based visibility of subcategories (added by later versions of the app).
ALTER TABLE categories ADD COLUMN IF NOT EXISTS visible_from TEXT;
ALTER TABLE categories ADD COLUMN IF NOT EXISTS archived_from TEXT;
UPDATE categories SET visible_from = substr(created_at, 1, 7) || '-01' WHERE parent_id IS NOT NULL AND (visible_from IS NULL OR visible_from = '');
CREATE INDEX IF NOT EXISTS idx_categories_parent_visible_archive ON categories(parent_id, visible_from, archived_from);

-- Every budget has its own categories. Categories that existed before budgets were separated
-- belong to the app owner's budget; subcategories follow their parent.
ALTER TABLE categories ADD COLUMN IF NOT EXISTS budget_id BIGINT;
UPDATE categories SET budget_id = (SELECT COALESCE(budget_owner_id, id) FROM users ORDER BY (role = 'owner') DESC, id ASC LIMIT 1)
  WHERE budget_id IS NULL AND parent_id IS NULL;
UPDATE categories c SET budget_id = p.budget_id FROM categories p WHERE c.parent_id = p.id AND c.budget_id IS NULL;
UPDATE categories c SET budget_id = p.budget_id FROM categories p WHERE c.parent_id = p.id AND c.budget_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_categories_budget ON categories(budget_id, parent_id);

-- Savings accounts with interest (src/savings.rs): settings live on a savings subcategory,
-- interest the server pays is marked with source = 'interest'.
-- 'interest_fixed' is a payment the user corrected, which is never recalculated.
ALTER TABLE categories ADD COLUMN IF NOT EXISTS acc_kind TEXT;
ALTER TABLE categories ADD COLUMN IF NOT EXISTS acc_term_days BIGINT;
ALTER TABLE categories ADD COLUMN IF NOT EXISTS acc_currency TEXT;
ALTER TABLE categories ADD COLUMN IF NOT EXISTS acc_start TEXT;
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS source TEXT;
-- Rates with the day each applies from, so a new rate doesn't change past months.
CREATE TABLE IF NOT EXISTS savings_rates (
  category_id BIGINT NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
  from_day TEXT NOT NULL,
  rate DOUBLE PRECISION NOT NULL,
  tax DOUBLE PRECISION NOT NULL DEFAULT 0,
  PRIMARY KEY (category_id, from_day)
);

-- A transfer between savings accounts is two operations pointing at each other: a withdrawal
-- from one and a deposit into the other, changed and deleted together.
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS pair_id BIGINT;

-- Sign-in with a login and password (src/local.rs). People without Telegram get
-- telegram_id = 'local:<login>'.
ALTER TABLE users ADD COLUMN IF NOT EXISTS login TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS password_hash TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_login ON users(login);
-- One-time links into a budget; only a hash of the link is kept.
CREATE TABLE IF NOT EXISTS invites (
  id BIGSERIAL PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  budget_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  created_by BIGINT,
  created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP::text),
  expires_at BIGINT NOT NULL,
  used_by BIGINT
);
