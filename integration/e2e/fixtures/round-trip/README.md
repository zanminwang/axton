# Round-trip fixture

The `Entry` / `Edit` schema, backend and console client behind [round-trip.test.mjs](../../round-trip.test.mjs). It was the first public example; the public example is now [examples/todo](../../../../examples/todo/README.md). The fixture stays because its assertions (normalization, rejection, lost-response retry, offline reopen, catch-up, Node and Dart clients) and the generic `Entry` snippets in the [documentation](../../../../website/docs/backend/api.md) depend on it.

`bash integration/e2e/run.sh` regenerates and installs it. `run.sh` here serves the backend alone for manual use of `client.mts` (`AXTON_URL`, `AXTON_DATABASE`, `edit TEXT`, `offline`, `online`, `status`, `quit`).
