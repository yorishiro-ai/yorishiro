-- Mode B (inferring missing values from an LLM) belongs to the hosted edition, not here.
--
-- It was built here first, on the reading that a BYO-key design carries no cost to the
-- deployment and so could live in the community edition. That reading was wrong about what
-- makes a feature enterprise: the deciding property is that the server calls an LLM at all,
-- not who pays for the call. The community edition makes no outbound model calls -- the
-- embedding providers are not this, since an embedding endpoint is not a chat completion.
--
-- The tables shipped in v0.40.0 and v0.41.0 as their own migrations, and again in v0.43.0
-- inside the consolidated initial. They are dropped rather than left in place because an
-- unused table holding a workspace's API key is worse than no table: nothing here reads it
-- any more, so nothing here would notice it going stale, and it would still be in every
-- backup.
--
-- Any workspace that stored a key loses it. The hosted edition creates both tables under its
-- own migrations; a key has to be set again there.
--
-- Numbered after 20260812100000_initial deliberately. The earlier attempt at this file was
-- 20260812010000, which sorts *before* the initial: `DROP TABLE IF EXISTS` would have found
-- nothing on a fresh database, the initial would then have created both tables, and the drop
-- would never run again. The suite passes either way -- no code queries these tables now --
-- so it would have looked finished while the key table survived. Creating and dropping within
-- one `migrate run` is harmless: no key can be written in that window.

DROP TABLE IF EXISTS content.fill_proposals;
DROP TABLE IF EXISTS identity.workspace_llm_keys;
