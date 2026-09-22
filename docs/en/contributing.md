# Contributing

## Tests

Use `make test-postgres` or `make test-sqlite` to run the integration suite.
Both commands keep shared tests in the run and verify that at least one test specific to the selected backend executes.
The final output reports selected, executed, and skipped backend-specific gate counts, including the reason for skipped gates.

Before pushing, run `make check-all`.
