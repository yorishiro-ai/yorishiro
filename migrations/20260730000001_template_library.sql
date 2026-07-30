-- User-contributed schema templates. Builtin templates (shipped with the binary)
-- remain in templates/*.json and are served from memory; this table holds templates
-- that users create and optionally share.
--
-- Templates are tenant-scoped: a tenant's members can see and use templates
-- published by that tenant. A future "community" visibility (cross-tenant) is
-- reserved but not enforced yet — the column exists so the schema doesn't need
-- another migration when it's implemented.
CREATE TABLE identity.templates (
    id          UUID PRIMARY KEY DEFAULT uuidv7(),
    tenant_id   UUID NOT NULL REFERENCES identity.tenants(id),
    name        TEXT NOT NULL,
    description TEXT,
    definition  JSONB NOT NULL,
    tags        TEXT[] NOT NULL DEFAULT '{}',
    locale      TEXT,
    visibility  TEXT NOT NULL DEFAULT 'tenant' CHECK (visibility IN ('tenant', 'community')),
    author      TEXT,
    fork_of     UUID REFERENCES identity.templates(id),
    created_by  UUID REFERENCES identity.users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, name)
);

CREATE INDEX templates_tenant_id_idx ON identity.templates(tenant_id);
CREATE INDEX templates_tags_idx ON identity.templates USING gin(tags);
