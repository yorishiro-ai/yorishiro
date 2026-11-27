# IAM and audit follow-up

- [x] Start new workspaces without a schema and reject unsupported creation inputs in REST and MCP.
- [x] Restrict updates and revocation of root credentials to active root sign-in sessions while preserving operator recovery.
- [x] Correlate REST and MCP requests and record authenticated identity without logging credentials or tool arguments.
- [x] Record workspace lifecycle changes and tenant updates, and include policy names in detachment events.
- [x] Remove the unused path type and correct stale comments without changing policy grants or combining distinct IAM actions.
- [x] Run the community and enterprise checks and backend suites after the changes.
- [x] Verify GitHub Actions on the updated pull request.
- [x] Initialize the local development database and verify startup.

# Release follow-up

- [x] Build stable dry runs with the requested version and publish the branch and tag only after verification.
- [x] Match Actions artifacts and release assets, and use the preview date tag in public filenames.
- [x] Verify release planning and artifact generation locally.
- [x] Share the release jobs and composite publishing action across both dispatch workflows.
- [x] Fix packaging assertions and document release recovery after the Actions review.
- [ ] Verify the release workflows end to end on GitHub Actions.
