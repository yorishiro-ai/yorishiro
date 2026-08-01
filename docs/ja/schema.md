# スキーマ定義

[English](../schema.md) | **日本語**

エンティティの型はJSONメタスキーマで定義します(例: `templates/task-management.json`):

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

- `type`: `string` / `integer` / `number` / `boolean` / `array` / `object`
- 制約: `required`、`enum`、`minimum`/`maximum`(number/integer)、`minLength`/`maxLength`/`pattern`(string)、`minItems`/`maxItems`/`uniqueItems`(array)、`format`(string限定。`date` / `date-time` / `uri` / `email` / `uuid`のいずれか)
- `array`フィールドは`items: { "type": ..., "properties": {...} }`が必要。アイテムの型は`string`または`object`(`object`の場合は自身の`properties`も必要)
- `object`フィールドは`properties: { ...FieldDef }`が必要で、最大5階層までネスト可能
- `description`はスキーマ・エンティティ型・リレーション型・フィールドのいずれのレベルでも省略可能
- フィールドの`x-ui`は任意のJSONオブジェクトで、UIヒントとしてそのまま保持される(例: `{"widget": "textarea"}`)
- `x-embed: true`を付けたフィールド(通常はstring)はベクトル埋め込みの対象になる。string以外の値は文字列化されてから使われる
- `relation_types`はエンティティ型どうしの有向リレーションを定義する
