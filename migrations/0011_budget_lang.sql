-- The language of a budget (src/db.rs, `names`): what the server names on its own — the first
-- categories, the main account, the interest subcategory — is in it. Budgets made before this
-- were Russian.
ALTER TABLE budget_settings ADD COLUMN lang TEXT NOT NULL DEFAULT 'ru' CHECK (lang IN ('ru', 'en', 'tr'));

-- pick_account() of 0009, naming a new main account in the budget's language.
CREATE OR REPLACE FUNCTION pick_account() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  b BIGINT;
  main TEXT;
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
    SELECT CASE s.lang WHEN 'en' THEN 'Main' WHEN 'tr' THEN 'Ana hesap' ELSE 'Основной' END INTO main
    FROM budget_settings s WHERE s.budget_id = b;
    INSERT INTO accounts (budget_id, name, emoji, currency, position)
    VALUES (b, COALESCE(main, 'Основной') || ' ' || NEW.currency, '💳', NEW.currency, (SELECT COALESCE(MAX(position), 0) + 1 FROM accounts WHERE budget_id = b))
    RETURNING id INTO NEW.account_id;
  END IF;
  RETURN NEW;
END
$$;
