-- Inferring missing values from an LLM is withdrawn: Yorishiro makes no outbound model calls.
--
-- It was built on the reading that a bring-your-own-key design carries no cost to the
-- deployment and so belonged here. That reading tested the wrong property -- what decides it
-- is that the server makes a chat completion at all, not who pays for the call. The embedding
-- providers are not the same thing and stay: an embeddings endpoint is not a chat completion,
-- and the local ONNX provider makes no network call whatsoever.
--
-- The tables shipped in v0.40.0 and v0.41.0 as their own migrations, and again in v0.43.0
-- inside the consolidated initial. They are dropped rather than left in place because an
-- unused table holding a workspace's API key is worse than no table: nothing reads it any
-- more, so nothing would notice it going stale, and it would still be in every backup.
--
-- Any workspace that stored a key loses it.
--
-- Numbered after 20260812100000_initial deliberately. The earlier attempt at this file was
-- 20260812010000, which sorts *before* the initial: `DROP TABLE IF EXISTS` would have found
-- nothing on a fresh database, the initial would then have created both tables, and the drop
-- would never run again. The suite passes either way -- no code queries these tables now --
-- so it would have looked finished while the key table survived. Creating and dropping within
-- one `migrate run` is harmless: no key can be written in that window.

DROP TABLE IF EXISTS content.fill_proposals;
DROP TABLE IF EXISTS identity.workspace_llm_keys;
