-- `identity.templates.fork_of` records which template a forked copy came from, purely so the
-- lineage stays traceable -- a fork is an independent copy, not a child that depends on its
-- parent's row. The initial schema left the FK at the default NO ACTION, which made deleting a
-- template that anything had been forked from fail with a foreign-key violation (surfacing as a
-- 500, since a constraint error isn't one of the cases `delete_template` maps to a 4xx).
--
-- SET NULL is the semantics the column already implies: deleting the parent drops the lineage
-- pointer and leaves the fork itself intact and usable.

ALTER TABLE identity.templates
  DROP CONSTRAINT templates_fork_of_fkey;

ALTER TABLE identity.templates
  ADD CONSTRAINT templates_fork_of_fkey
  FOREIGN KEY (fork_of) REFERENCES identity.templates(id) ON DELETE SET NULL;
