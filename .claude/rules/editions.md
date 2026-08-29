# Editions and the `ee/` boundary

- One repository, two licences.
  Everything outside `ee/` is BUSL-1.1; `ee/` is the paid edition under `ee/LICENSE`, which adds a Competing Use restriction and requires a licence key for production use.
- **`src/app.rs` is the only file in `src/` that may reference `crate::ee`. Everywhere else the dependency runs from `ee/` into `src/`, never back.**
  `app.rs` is the composition root: `after_context` installs `ee/`'s seams and `routes()` mounts its routes, which is wiring rather than a feature depending on a feature.
  One crate makes both directions compile, so this is a rule rather than a compiler error: `use crate::ee::...` in any other `src/` file is the import direction that is always wrong.
- Which side a feature belongs on is decided by what the feature *is*, never by what it needs.
  **The server calling an LLM**, billing, external SaaS and rich UI are `ee/` by character.
  "The user brings their own key" does not move a feature out of `ee/`, because it changes who pays rather than what the server does.
  Any sentence of the form "it does not depend on X" is the wrong test.
- Unclear cases are a question for the user, asked as the classification itself rather than buried in the options of an implementation question.

See `ee-composition.md` for how `ee/` is wired in and where the licence gate sits.
