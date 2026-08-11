-- The definition as it stood when the copy was taken.
--
-- Following an upstream edit is a three-way question: what the template said then, what it
-- says now, and what this workspace has done to its copy since. Without the first, an edit
-- upstream and an edit here are indistinguishable — both look like "differs from the
-- template" — and merging cannot tell an addition to accept from a local change to keep.
--
-- Stored rather than reconstructed: the template's own history is not kept, so there is
-- nowhere to look this up after the fact.
ALTER TABLE content.schemas
  ADD COLUMN origin_snapshot JSONB;

-- Only rows that follow a template have one, and only from here on. Existing rows keep NULL:
-- what their template said at the time is not recoverable, and inventing it would produce a
-- merge base that never existed.
