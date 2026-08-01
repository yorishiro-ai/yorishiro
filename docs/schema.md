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
