# OpenCode Go spike fixtures

These are deterministic, synthetic fixtures for issue #33. They are not
captured account payloads and must not be treated as a substitute for the
live authenticated API response.

The original fixture shapes mirror the field names in the historical upstream
proposal [opencode/opencode#16513](https://github.com/anomalyco/opencode/pull/16513).
The production adapter tests cover the deployed `usage.*.percent` and
`resetsAt` fields directly; these files remain useful for documenting failure
categories without storing credentials. No file contains an API key, cookie,
email, account id, or raw browser response.
