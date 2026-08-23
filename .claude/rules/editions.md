# Editions and the `ee/` boundary

- One repository, two licences.
  Everything outside `ee/` is BUSL-1.1; `ee/` is the paid edition under `ee/LICENSE`, which adds a Competing Use restriction and requires a licence key for production use.
- **The root `yorishiro-core` app crate must not depend on `ee/`. `ee/` depends on it.**
  One binary composes both.
  A `use` or a path dependency pointing from the root crate into `ee/` inverts this and is the one import direction that is always wrong.
- Which side a feature belongs on is decided by what the feature *is*, never by what it needs.
  **The server calling an LLM**, billing, external SaaS and rich UI are `ee/` by character.
  "The user brings their own key" does not move a feature out of `ee/`, because it changes who pays rather than what the server does.
  Any sentence of the form "it does not depend on X" is the wrong test.
- Unclear cases are a question for the user, asked as the classification itself rather than buried in the options of an implementation question.

See `ee-composition.md` for how `ee/` composes on top of the Loco rebuild specifically.
