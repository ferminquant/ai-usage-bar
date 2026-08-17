# OpenCode Go spike fixtures

These are deterministic, synthetic fixtures for issue #33. They are not
captured account payloads and must not be treated as a released API schema.

The `normal.json` and `missing_windows.json` shapes mirror the field names in
the still-open upstream proposal [opencode/opencode#16513](https://github.com/anomalyco/opencode/pull/16513).
The error fixtures represent the failure states an eventual adapter must
handle. No file contains an API key, cookie, email, account id, or raw browser
response.
