-- A recurring operation remembers the account it is paid from or into (src/recurring.rs);
-- each operation it adds is booked there (the trigger of 0005 still checks the currency).
ALTER TABLE recurring ADD COLUMN account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL;
