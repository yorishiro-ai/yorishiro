# IAM and audit follow-up

- [x] Start new workspaces without a schema and reject unsupported creation inputs in REST and MCP.
- [x] Restrict updates and revocation of root credentials to active root sign-in sessions while preserving operator recovery.
- [x] Correlate REST and MCP requests and record authenticated identity without logging credentials or tool arguments.
- [x] Record workspace lifecycle changes and tenant updates, and include policy names in detachment events.
- [x] Remove the unused path type and correct stale comments without changing policy grants or combining distinct IAM actions.
- [ ] Run the community and enterprise checks and backend suites after the changes.
- [ ] Verify GitHub Actions on the updated pull request.
- [ ] Initialize the local development database and verify startup.
