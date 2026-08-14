-- OAuth2/OIDC support for enterprise. This is an enterprise-only migration, kept in this repo
-- rather than in the vendored `vendor/yorishiro/migrations` (community edition, out of our
-- control) -- `yorishiro-hosted-server`'s `main` runs both migration directories against the
-- same database, one after the other. Sqlx tracks applied versions in a single
-- `_sqlx_migrations` table keyed by the numeric prefix, so interleaving two directories' version
-- numbers is safe as long as this file's timestamp sorts after every community migration it
-- depends on (it depends on `identity.users` from `20260712000001_initial.sql`).
--
-- An OAuth-provisioned user has no password of their own -- they authenticate entirely through
-- the identity provider -- so `password_hash` has to become nullable rather than requiring a
-- dummy value. `identity.users` rows created via `POST /auth/signup` (or `admin create-invite`)
-- are unaffected: `password_hash` stays required for them by the CHECK constraint below, which
-- ties nullability to `oauth_provider` being set instead of dropping the NOT NULL unconditionally.
ALTER TABLE identity.users
  ALTER COLUMN password_hash DROP NOT NULL;

ALTER TABLE identity.users
  ADD COLUMN oauth_provider   TEXT,
  ADD COLUMN oauth_subject_id TEXT;

-- Every row is either password-authenticated (password_hash set, oauth_* both NULL) or
-- OAuth-provisioned (oauth_provider + oauth_subject_id set, password_hash may be NULL) -- never
-- a mix, and never neither (a user login method must be determinable at a glance).
ALTER TABLE identity.users
  ADD CONSTRAINT users_auth_method_check CHECK (
    (password_hash IS NOT NULL AND oauth_provider IS NULL AND oauth_subject_id IS NULL)
    OR (oauth_provider IS NOT NULL AND oauth_subject_id IS NOT NULL)
  );

-- The subject id ("sub" claim) an identity provider issues is only unique within that provider,
-- so the lookup/uniqueness key is the pair, not either column alone -- otherwise two different
-- providers that happen to both hand out subject id "1" would collide.
CREATE UNIQUE INDEX users_oauth_identity_idx
  ON identity.users (oauth_provider, oauth_subject_id)
  WHERE oauth_provider IS NOT NULL;
