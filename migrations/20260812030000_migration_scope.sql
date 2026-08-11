-- `migration`, a scope above `schema`.
--
-- A schema registration adds a version nothing has been written against yet; running a batch
-- migration rewrites stored rows, and switching maintenance mode stops every caller. Those two
-- are what this scope guards, and they are a different kind of authority from registering a
-- definition -- which is why they no longer sit under `schema`.
--
-- `audit` (the other half of the spec's hierarchy) is deliberately absent: a scope over an audit
-- log that does not exist yet would read as though the log did.
ALTER TABLE identity.api_keys DROP CONSTRAINT IF EXISTS api_keys_scope_check;
ALTER TABLE identity.api_keys
  ADD CONSTRAINT api_keys_scope_check CHECK (scope IN ('read', 'write', 'schema', 'migration'));
