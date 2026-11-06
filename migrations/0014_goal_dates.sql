-- The month a savings goal is meant to be reached in (its first day): the budget's goal and a
-- savings account's own one. NULL means no date.
ALTER TABLE budget_settings ADD COLUMN IF NOT EXISTS savings_goal_date TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS target_date TEXT;
