-- An operation booked without an account (an import, a recurring payment with none, a changed
-- currency) goes to an everyday account, never to a savings one: those are paid into by
-- transfers. The account given is still kept when its currency fits, savings ones included
-- (interest is income booked on the savings account itself).
CREATE OR REPLACE FUNCTION pick_account() RETURNS trigger LANGUAGE plpgsql AS $$
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
  -- One budget at a time, so two operations at once don't both make a main account.
  PERFORM pg_advisory_xact_lock(7001, b::int);  -- 7001: making accounts (src/accounts.rs)
  SELECT a.id INTO NEW.account_id FROM accounts a
  WHERE a.budget_id = b AND a.currency = NEW.currency AND a.kind = 'regular' AND NOT a.archived
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
