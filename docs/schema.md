# Schema Definition

**English** | [日本語](ja/schema.md)

Entity types are defined with a JSON meta-schema (example: `templates/task-management.json`):

```json
{
  "name": "task-management",
  "entity_types": {
    "task": {
      "fields": {
        "title": { "type": "string", "required": true, "x-embed": true },
        "done":  { "type": "boolean", "default": false }
      }
    },
    "project": {
      "fields": { "title": { "type": "string", "required": true } }
    }
  },
  "relation_types": {
    "belongs_to": { "source": "task", "target": "project" }
  }
}
```

- `type`: one of `string` / `integer` / `number` / `boolean` / `array` / `object`
- Constraints: `required`, `enum`, `minimum`/`maximum` (number/integer), `minLength`/`maxLength`/`pattern` (string), `minItems`/`maxItems`/`uniqueItems` (array), `format` (string; one of `date` / `date-time` / `uri` / `email` / `uuid`)
- `array` fields need `items: { "type": ..., "properties": {...} }` -- item type is `string` or `object` (with `object` items needing their own `properties`)
- `object` fields need `properties: { ...FieldDef }`, nesting up to 5 levels deep
- `description` is optional at every level (schema, entity type, relation type, field)
- `x-ui` on a field is an arbitrary JSON object of UI hints, preserved as-is (e.g. `{"widget": "textarea"}`)
- A field marked `x-embed: true` (typically `string`) becomes a target for vector embedding; non-string values are stringified first
- `relation_types` defines directed relations between entity types
- A schema created from a library template records which one in `origin_template_id`, with `origin_status: "linked"`.
  Deleting that template does not touch the copy — it only clears the link and moves the schema to `"detached"`, so a template can be withdrawn without breaking the workspaces using it.
  A schema written by hand is `"detached"` from the start, having never had an origin.

## Compatible and breaking changes

Updating a schema diffs the new definition against the current one.
A compatible change updates the existing version in place; a breaking change creates a new version, leaving records written under the old one still readable at their own version.

A change is **breaking** when data that was valid a moment ago would no longer validate:

- removing or renaming a field, an entity type, or a relation type
- changing a field's type, or an array field's item type
- making an existing field required, or adding a new field that is required
- changing a relation type's source or target
- removing a value from an `enum` that still constrains the field
- tightening a constraint: adding one where there was none, raising `minimum` / `minLength` / `minItems`, lowering `maximum` / `maxLength` / `maxItems`, changing `format` or `pattern` from one value to another, or turning `uniqueItems` on

A change is **compatible** when it can only admit data that was previously rejected:

- adding an optional field, an entity type, or a relation type
- adding a value to an `enum`, or dropping the `enum` constraint entirely
- loosening a constraint: dropping one, lowering `minimum` / `minLength` / `minItems`, raising `maximum` / `maxLength` / `maxItems`, or turning `uniqueItems` off
- editing any `description`, or a field's `x-ui` hints
