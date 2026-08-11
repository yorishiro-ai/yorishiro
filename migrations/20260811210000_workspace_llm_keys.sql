-- Per-workspace credentials for the one feature that calls an LLM: inferring values for fields
-- an entity is missing.
--
-- This is the first secret in the schema that has to be read back. API keys are stored as a
-- hash and compared; the embedding provider's key and Stripe's come from the environment and
-- belong to the deployment. This one belongs to a workspace and has to leave the process as an
-- Authorization header, so it is kept as written.
--
-- The protection is the same one `identity.tenants` and `identity.api_keys` get: no GRANT to
-- `yorishiro_app`. The request role cannot read this table at all, so no RLS policy can be the
-- thing that fails. Reads go through the migration-role pool the admin CLI and the identity
-- endpoints already use.
--
-- Not encrypted at rest. A key in an environment variable would protect against a stolen
-- database dump, but in a self-hosted deployment that variable sits on the same host as the
-- database, so the dump and the key are taken together -- and losing the variable would make
-- every workspace's key unreadable at once. Operators who need encryption at rest have it
-- below this layer, in the volume or the managed database.
CREATE TABLE identity.workspace_llm_keys (
  workspace_id UUID PRIMARY KEY REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  -- OpenAI-compatible chat completions. The same shape the embedding provider already speaks,
  -- so an operator pointing at Ollama or LM Studio configures it the same way.
  base_url     TEXT NOT NULL,
  model        TEXT NOT NULL,
  api_key      TEXT NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
