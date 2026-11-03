-- Two-factor sign-in with a code from an authenticator app (src/totp.rs).
-- totp_secret is set once the first code was confirmed; totp_pending holds a secret being set up.
-- totp_step is the last time step used, so a code works only once.
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_pending TEXT;
ALTER TABLE users ADD COLUMN totp_step BIGINT;
-- One-time codes for when the phone is lost; only their hashes are kept.
CREATE TABLE recovery_codes (
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  code_hash TEXT NOT NULL,
  PRIMARY KEY (user_id, code_hash)
);
