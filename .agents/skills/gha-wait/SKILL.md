---
name: gha-wait
description: Use when waiting for GitHub Actions or pull request CI after a push; register a gh wait rule instead of polling or repeatedly replying that CI is pending.
---

# Wait For GitHub Actions

Follow `.agents/rules/git-workflow.md`.
Use the installed `gh-wait` GitHub CLI extension; if unavailable, ask before installing it.

1. Identify the workflow run for the pushed commit with `gh run list` and verify its `headSha`.
2. Register one rule with `gh wait workflow <run-id> --repo <owner/repo> --completed`.
3. The command returns after registration, not after CI completion.
   Do independent work or yield until a completion event arrives.
   Do not poll, create duplicate watches, use `gh run watch` or `gh pr checks --watch`, or repeatedly send waiting replies.
4. After the event, inspect `gh run view <run-id>` and `gh run view <run-id> --log-failed` when needed.
   Verify the commit and actual matrix results before claiming success.

Use `gh wait list` to inspect existing registrations when needed and delete only a rule you own.
Never use `gh wait delete --all` or stop the shared server to clean up one task.
The default action logs completion to the gh-wait server; do not assume it automatically resumes the agent.
If the harness cannot deliver that event, explain this limitation once rather than starting a polling loop.
