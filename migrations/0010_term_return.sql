-- A term deposit either renews with its interest (as before) or ends: the bank then returns the
-- money with the interest to `payout_account_id` (src/savings.rs makes that transfer), and a
-- new term is opened by hand.
ALTER TABLE accounts ADD COLUMN term_renew BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE accounts ADD COLUMN payout_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL;
