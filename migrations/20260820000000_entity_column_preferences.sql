-- Which columns the Entities table shows, per workspace.
--
-- The create form is already schema-driven, so the table beside it showing a fixed four columns
-- (name, type, data preview, created) is the half that never caught up. A workspace whose schema
-- defines `status` and `priority` cannot see either without opening a row.
--
-- Scoped to the workspace rather than the user: everyone looking at a workspace sees the same
-- table, which is what makes a shared view shared. A per-user override would need a second
-- nullable `user_id` column and a lookup that prefers the user's row over the workspace's, and
-- that is a change to this table rather than a different one.
--
-- One row per (workspace, entity_type). Entity types within a workspace have different fields,
-- so one row per workspace would make the setting meaningless the moment a schema declares two.

CREATE TABLE content.entity_column_preferences (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  entity_type  TEXT NOT NULL,
  -- Field names from the schema's `fields`, in the order they should be displayed.
  -- A name that is no longer in the schema is ignored on read rather than cleaned up on write:
  -- a schema migration would otherwise have to know about display settings.
  columns      JSONB NOT NULL DEFAULT '[]'::jsonb,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

  -- The upsert target. Without it, two tabs saving at once leave two rows and the reader picks
  -- one arbitrarily.
  UNIQUE (workspace_id, entity_type),

  -- An array, not an object: the order is the display order, and a JSONB object does not keep
  -- insertion order.
  CONSTRAINT entity_column_preferences_columns_is_array
    CHECK (jsonb_typeof(columns) = 'array')
);

-- Every read is "this workspace, this entity type", which the unique constraint already indexes.
-- No second index: one row per entity type per workspace is a handful of rows.

ALTER TABLE content.entity_column_preferences ENABLE ROW LEVEL SECURITY;

-- Read leniently, matching `content.fill_proposals`: the control-plane pool reaches this over a
-- connection that names no workspace.
CREATE POLICY workspace_isolation ON content.entity_column_preferences
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE ON content.entity_column_preferences TO yorishiro_app;
