-- Savings become accounts (src/accounts.rs, src/savings.rs): an account of kind 'savings' has a
-- starting balance like any other, is paid into and out of by transfers from any account, and may
-- earn interest. Interest is income ("Проценты") booked on the account. The savings categories
-- stay in the database, empty, for older apps; nothing new is booked on them.
ALTER TABLE accounts ADD COLUMN kind TEXT NOT NULL DEFAULT 'regular' CHECK (kind IN ('regular', 'savings'));
ALTER TABLE accounts ADD COLUMN target_amount DOUBLE PRECISION;
ALTER TABLE accounts ADD COLUMN interest_kind TEXT CHECK (interest_kind IN ('flexible', 'term'));
ALTER TABLE accounts ADD COLUMN term_days BIGINT;
ALTER TABLE accounts ADD COLUMN interest_start TEXT;
-- The savings subcategory an account was made from, for older apps that still name it.
ALTER TABLE accounts ADD COLUMN legacy_category_id BIGINT REFERENCES categories(id) ON DELETE SET NULL;
-- Interest rates with the day each applies from, as savings_rates had them per subcategory.
CREATE TABLE account_rates (
  account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  from_day TEXT NOT NULL,
  rate DOUBLE PRECISION NOT NULL,
  tax DOUBLE PRECISION NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, from_day)
);
-- The budget's savings goal, in its main currency; each savings account may have its own too.
ALTER TABLE budget_settings ADD COLUMN savings_goal DOUBLE PRECISION;
-- A recurring transfer (a monthly deposit into savings): from account_id into to_account_id.
ALTER TABLE recurring ADD COLUMN to_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL;
ALTER TABLE recurring DROP CONSTRAINT recurring_type_check;
ALTER TABLE recurring ADD CONSTRAINT recurring_type_check CHECK (type IN ('income', 'expense', 'savings', 'transfer'));

-- Moving the data. Every savings category (the root and its subcategories) becomes one savings
-- account per currency it holds. Their operations become:
--   the starting balance («Первоначальная сумма…») → the account's starting balance;
--   a deposit → a transfer from the account it was paid from into the savings account;
--   a withdrawal → a transfer from the savings account into the account it went to;
--   a transfer between savings subcategories → a transfer between their accounts;
--   interest → income «Проценты» on the savings account.
CREATE TEMP TABLE savings_map (category_id BIGINT, currency TEXT, account_id BIGINT, PRIMARY KEY (category_id, currency));

DO $$
DECLARE
  sc RECORD;
  cur TEXT;
  n INT;
  acc BIGINT;
  op RECORD;
  partner RECORD;
  regular BIGINT;
  outer_id BIGINT;
  inner_id BIGINT;
  income_root BIGINT;
  pct BIGINT;
BEGIN
  FOR sc IN SELECT * FROM categories WHERE type = 'savings' AND budget_id IS NOT NULL ORDER BY (parent_id IS NOT NULL), id LOOP
    SELECT count(*) INTO n FROM (
      SELECT currency FROM transactions WHERE category_id = sc.id AND type = 'savings'
      UNION SELECT sc.acc_currency WHERE sc.acc_currency IS NOT NULL) q;
    FOR cur IN SELECT x FROM (
      SELECT currency AS x FROM transactions WHERE category_id = sc.id AND type = 'savings'
      UNION SELECT sc.acc_currency WHERE sc.acc_currency IS NOT NULL) q WHERE x IS NOT NULL ORDER BY x LOOP
      INSERT INTO accounts (budget_id, name, emoji, currency, kind, target_amount, interest_kind, term_days, interest_start,
                            legacy_category_id, archived, position)
      VALUES (sc.budget_id, sc.name || CASE WHEN n > 1 THEN ' ' || cur ELSE '' END, '🎯', cur, 'savings',
              CASE WHEN sc.parent_id IS NOT NULL THEN sc.target_amount END,
              CASE WHEN sc.acc_currency = cur THEN sc.acc_kind END,
              CASE WHEN sc.acc_currency = cur THEN sc.acc_term_days END,
              CASE WHEN sc.acc_currency = cur THEN sc.acc_start END,
              sc.id, sc.archived_from IS NOT NULL, 1000 + sc.id)
      RETURNING id INTO acc;
      INSERT INTO savings_map VALUES (sc.id, cur, acc);
      IF sc.acc_currency = cur THEN
        INSERT INTO account_rates (account_id, from_day, rate, tax) SELECT acc, from_day, rate, tax FROM savings_rates WHERE category_id = sc.id;
      END IF;
    END LOOP;
    IF sc.parent_id IS NULL AND sc.target_amount > 0 THEN
      INSERT INTO budget_settings (budget_id, savings_goal) VALUES (sc.budget_id, sc.target_amount)
      ON CONFLICT (budget_id) DO UPDATE SET savings_goal = EXCLUDED.savings_goal;
    END IF;
  END LOOP;

  -- Transfers between savings subcategories: one pass over their withdrawal halves.
  FOR op IN SELECT t.*, m.account_id AS acc FROM transactions t JOIN savings_map m ON m.category_id = t.category_id AND m.currency = t.currency
            WHERE t.type = 'savings' AND t.pair_id IS NOT NULL AND t.source = 'withdrawal' LOOP
    SELECT t.*, m.account_id AS acc INTO partner FROM transactions t JOIN savings_map m ON m.category_id = t.category_id AND m.currency = t.currency
    WHERE t.id = op.pair_id;
    IF partner.acc IS NOT NULL THEN
      INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, created_at)
      VALUES (op.user_id, op.acc, 'transfer', op.amount, op.currency, op.note, op.transaction_date, 'transfer_out', op.created_at) RETURNING id INTO outer_id;
      INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, created_at)
      VALUES (partner.user_id, partner.acc, 'transfer', partner.amount, partner.currency, partner.note, partner.transaction_date, 'transfer_in', partner.created_at) RETURNING id INTO inner_id;
      UPDATE transactions SET pair_id = inner_id WHERE id = outer_id;
      UPDATE transactions SET pair_id = outer_id WHERE id = inner_id;
      DELETE FROM transactions WHERE id IN (op.id, partner.id);
    END IF;
  END LOOP;

  -- Deposits and withdrawals: the other end is the regular account they were booked on.
  FOR op IN SELECT t.*, m.account_id AS acc, c.budget_id AS budget FROM transactions t
            JOIN savings_map m ON m.category_id = t.category_id AND m.currency = t.currency
            JOIN categories c ON c.id = t.category_id
            WHERE t.type = 'savings' AND t.pair_id IS NULL AND (t.source IS NULL OR t.source = 'withdrawal')
              AND COALESCE(t.note, '') NOT LIKE 'Первоначальная сумма%' LOOP
    regular := op.account_id;
    IF regular IS NULL OR NOT EXISTS (SELECT 1 FROM accounts WHERE id = regular AND kind = 'regular') THEN
      SELECT id INTO regular FROM accounts WHERE budget_id = op.budget AND kind = 'regular' AND currency = op.currency AND NOT archived ORDER BY position, id LIMIT 1;
      IF regular IS NULL THEN
        INSERT INTO accounts (budget_id, name, emoji, currency) VALUES (op.budget, 'Основной ' || op.currency, '💳', op.currency) RETURNING id INTO regular;
      END IF;
    END IF;
    INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, created_at)
    VALUES (op.user_id, CASE WHEN op.source = 'withdrawal' THEN op.acc ELSE regular END, 'transfer', op.amount, op.currency, op.note, op.transaction_date, 'transfer_out', op.created_at)
    RETURNING id INTO outer_id;
    INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, created_at)
    VALUES (op.user_id, CASE WHEN op.source = 'withdrawal' THEN regular ELSE op.acc END, 'transfer', op.amount, op.currency, op.note, op.transaction_date, 'transfer_in', op.created_at)
    RETURNING id INTO inner_id;
    UPDATE transactions SET pair_id = inner_id WHERE id = outer_id;
    UPDATE transactions SET pair_id = outer_id WHERE id = inner_id;
    DELETE FROM transactions WHERE id = op.id;
  END LOOP;

  -- Interest: income «Проценты» of the budget, on the savings account.
  FOR op IN SELECT t.id, m.account_id AS acc, c.budget_id AS budget FROM transactions t
            JOIN savings_map m ON m.category_id = t.category_id AND m.currency = t.currency
            JOIN categories c ON c.id = t.category_id
            WHERE t.type = 'savings' AND t.source IN ('interest', 'interest_fixed') LOOP
    SELECT id INTO income_root FROM categories WHERE budget_id = op.budget AND type = 'income' AND parent_id IS NULL ORDER BY id LIMIT 1;
    IF income_root IS NULL THEN
      INSERT INTO categories (name, emoji, color, type, budget_id) VALUES ('Доход', '💰', '#9ece6a', 'income', op.budget) RETURNING id INTO income_root;
    END IF;
    SELECT id INTO pct FROM categories WHERE parent_id = income_root AND name = 'Проценты' ORDER BY id LIMIT 1;
    IF pct IS NULL THEN
      INSERT INTO categories (name, emoji, color, type, parent_id, budget_id, visible_from)
      VALUES ('Проценты', '•', '#545c7e', 'income', income_root, op.budget, '2000-01-01') RETURNING id INTO pct;
    END IF;
    UPDATE transactions SET type = 'income', category_id = pct, account_id = op.acc WHERE id = op.id;
  END LOOP;

  -- Starting balances, last: the day stays only if nothing on the account is dated earlier
  -- (else those operations would drop out of the balance); 2000-01-01 meant "no day".
  FOR op IN SELECT t.*, m.account_id AS acc FROM transactions t JOIN savings_map m ON m.category_id = t.category_id AND m.currency = t.currency
            WHERE t.type = 'savings' AND COALESCE(t.note, '') LIKE 'Первоначальная сумма%' ORDER BY t.transaction_date LOOP
    UPDATE accounts a SET
      initial_amount = a.initial_amount + op.amount,
      initial_date = CASE
        WHEN a.initial_amount <> 0 OR op.transaction_date <= '2000-01-01'
             OR EXISTS (SELECT 1 FROM transactions x WHERE x.account_id = a.id AND x.transaction_date < op.transaction_date) THEN NULL
        ELSE op.transaction_date END
    WHERE a.id = op.acc;
    DELETE FROM transactions WHERE id = op.id;
  END LOOP;

  -- Recurring deposits into savings become recurring transfers.
  UPDATE recurring r SET type = 'transfer', to_account_id = m.account_id
  FROM savings_map m WHERE r.type = 'savings' AND m.category_id = r.category_id AND m.currency = r.currency;
END
$$;
DROP TABLE savings_map;
