-- Accounts (src/accounts.rs): where money is, a card or cash, in one currency. A savings
-- account with interest stays a savings subcategory (src/savings.rs); the app lists both.
-- `owner_id` names whose account it is; NULL is shared. Everyone in the budget sees all.
-- The balance counts from `initial_date` (all operations when NULL) on top of `initial_amount`.
CREATE TABLE accounts (
  id BIGSERIAL PRIMARY KEY,
  budget_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  emoji TEXT NOT NULL DEFAULT '💳',
  currency TEXT NOT NULL,
  owner_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  initial_amount DOUBLE PRECISION NOT NULL DEFAULT 0,
  initial_date TEXT,
  archived BOOLEAN NOT NULL DEFAULT FALSE,
  position BIGINT NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_accounts_budget ON accounts(budget_id);

-- Income and expenses are paid from an account; a savings deposit says which account the
-- money left, a withdrawal where it went. A transfer between accounts is two operations
-- of type 'transfer' pointing at each other (pair_id), source 'transfer_out' / 'transfer_in',
-- each in its own account's currency, and with no category: it is neither spent nor earned.
ALTER TABLE transactions ADD COLUMN account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL;
CREATE INDEX idx_transactions_account ON transactions(account_id, transaction_date);
ALTER TABLE transactions DROP CONSTRAINT transactions_type_check;
ALTER TABLE transactions ADD CONSTRAINT transactions_type_check CHECK (type IN ('income', 'expense', 'savings', 'transfer'));

-- Money that existed before accounts: one main account per budget and currency it was in.
-- Interest, starting balances of savings and transfers between savings accounts stay
-- inside savings and need none.
INSERT INTO accounts (budget_id, name, emoji, currency)
SELECT DISTINCT c.budget_id, 'Основной', '💳', t.currency
FROM transactions t JOIN categories c ON c.id = t.category_id
WHERE c.budget_id IS NOT NULL
  AND (t.type IN ('income', 'expense')
       OR (t.type = 'savings' AND t.pair_id IS NULL AND (t.source IS NULL OR t.source = 'withdrawal')
           AND COALESCE(t.note, '') NOT LIKE 'Первоначальная сумма%'));
UPDATE accounts a SET name = 'Основной ' || a.currency
WHERE (SELECT count(*) FROM accounts b WHERE b.budget_id = a.budget_id) > 1;
UPDATE transactions t SET account_id = a.id
FROM categories c, accounts a
WHERE c.id = t.category_id AND a.budget_id = c.budget_id AND a.currency = t.currency
  AND (t.type IN ('income', 'expense')
       OR (t.type = 'savings' AND t.pair_id IS NULL AND (t.source IS NULL OR t.source = 'withdrawal')
           AND COALESCE(t.note, '') NOT LIKE 'Первоначальная сумма%'));

-- Every operation that moves money gets an account of its currency, however it was added
-- (the app, recurring payments, an import, an older app): the one given if its currency fits,
-- else the author's own account, else a shared one, else a new main account of that currency.
CREATE FUNCTION pick_account() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  b BIGINT;
BEGIN
  IF NEW.type = 'transfer' THEN
    RETURN NEW;
  END IF;
  IF NOT (NEW.type IN ('income', 'expense')
          OR (NEW.type = 'savings' AND NEW.pair_id IS NULL AND (NEW.source IS NULL OR NEW.source = 'withdrawal')
              AND COALESCE(NEW.note, '') NOT LIKE 'Первоначальная сумма%')) THEN
    NEW.account_id := NULL;
    RETURN NEW;
  END IF;
  SELECT budget_id INTO b FROM categories WHERE id = NEW.category_id;
  IF b IS NULL THEN
    RETURN NEW;
  END IF;
  IF NEW.account_id IS NOT NULL
     AND EXISTS (SELECT 1 FROM accounts WHERE id = NEW.account_id AND budget_id = b AND currency = NEW.currency) THEN
    RETURN NEW;
  END IF;
  SELECT a.id INTO NEW.account_id FROM accounts a
  WHERE a.budget_id = b AND a.currency = NEW.currency AND NOT a.archived
  ORDER BY (a.owner_id IS NOT DISTINCT FROM NEW.user_id) DESC, (a.owner_id IS NULL) DESC, a.position, a.id
  LIMIT 1;
  IF NEW.account_id IS NULL THEN
    INSERT INTO accounts (budget_id, name, emoji, currency, position)
    VALUES (b, 'Основной ' || NEW.currency, '💳', NEW.currency, (SELECT COALESCE(MAX(position), 0) + 1 FROM accounts WHERE budget_id = b))
    RETURNING id INTO NEW.account_id;
  END IF;
  RETURN NEW;
END
$$;
CREATE TRIGGER transactions_pick_account
  BEFORE INSERT OR UPDATE OF currency, account_id, type, source, note, pair_id, category_id ON transactions
  FOR EACH ROW EXECUTE FUNCTION pick_account();
