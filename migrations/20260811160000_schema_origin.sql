-- Where a schema came from.
--
-- Applying a template copies its definition, and the copy has kept no record of that. This
-- matters when the template later changes or disappears: without a link there is nothing to
-- notify, and without a state there is no way to say the link is gone.
--
-- Both columns are nullable. A schema written by hand has no origin, which is not a defect.
ALTER TABLE content.schemas
  ADD COLUMN origin_template_id UUID REFERENCES identity.templates(id) ON DELETE SET NULL,
  ADD COLUMN origin_status TEXT NOT NULL DEFAULT 'detached'
    CHECK (origin_status IN ('linked', 'detached'));

-- Existing rows: 'detached' is the honest answer. They were created before the link existed,
-- and claiming they follow a template nobody recorded would invent a relationship.
--
-- 'detached' is also the default for new rows. Only the template path sets 'linked', so a
-- schema posted as a bare definition does not accidentally claim an origin.

-- ON DELETE SET NULL above drops the reference when the template goes; this drops the claim to
-- be following it. Without the trigger a yanked template leaves origin_status = 'linked' with
-- origin_template_id = NULL -- a row that says it follows something unnameable.
--
-- Done in the database rather than in the application because the delete can arrive from the
-- admin CLI or a migration, neither of which passes through the code that would maintain it.
CREATE FUNCTION content.detach_orphaned_schema_origin() RETURNS TRIGGER AS $$
BEGIN
  UPDATE content.schemas
     SET origin_status = 'detached'
   WHERE origin_template_id = OLD.id
     AND origin_status = 'linked';
  RETURN OLD;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

CREATE TRIGGER templates_detach_schema_origins
  BEFORE DELETE ON identity.templates
  FOR EACH ROW EXECUTE FUNCTION content.detach_orphaned_schema_origin();

CREATE INDEX schemas_origin_template_idx
  ON content.schemas (origin_template_id)
  WHERE origin_template_id IS NOT NULL;
