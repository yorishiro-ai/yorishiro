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

## 互換な変更と破壊的な変更

スキーマを更新すると、新しい定義が現行の定義と差分比較される。互換な変更は既存バージョンを
その場で更新し、破壊的な変更は新しいバージョンを作る。旧バージョンで書かれたレコードは
それぞれのバージョンのまま読み出せる。

**破壊的**なのは、直前まで妥当だったデータが妥当でなくなる変更である:

- フィールド・エンティティ型・リレーション型の削除またはリネーム
- フィールドの型変更、配列フィールドのアイテム型変更
- 既存フィールドを必須にする、必須フィールドを新規追加する
- リレーション型の source / target の変更
- フィールドに残っている `enum` から値を削除する
- 制約の厳格化: 制約が無かったところへ追加する、`minimum` / `minLength` / `minItems` を
  引き上げる、`maximum` / `maxLength` / `maxItems` を引き下げる、`format` や `pattern` を
  別の値へ変更する、`uniqueItems` を有効にする

**互換**なのは、これまで弾かれていたデータを受け入れるようにするだけの変更である:

- 省略可能なフィールド・エンティティ型・リレーション型の追加
- `enum` への値の追加、`enum` 制約そのものの撤廃
- 制約の緩和: 制約の撤廃、`minimum` / `minLength` / `minItems` の引き下げ、
  `maximum` / `maxLength` / `maxItems` の引き上げ、`uniqueItems` の無効化
- `description` やフィールドの `x-ui` ヒントの編集
