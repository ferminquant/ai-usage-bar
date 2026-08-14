# OpenCode Go spike fixtures

These are deterministic, synthetic fixtures for issue #33. They are not
captured account payloads and must not be treated as a released API schema.

The `normal.json` and `missing_*.json` shapes mirror the field names in the
upstream proposal [opencode/opencode#16513](https://github.com/anomalyco/opencode/pull/16513),
which is now **merged**: the `GET /zen/go/v1/usage` route responds HTTP 401
behind key auth in a 2026-08-14 probe but is still not documented as a
released contract. The fixtures therefore remain dated planning artifacts for
that unreleased shape, and `unavailable.json` is a **dated 2026-08-06
capture** of the live HTTP 404 observed before the route was deployed. The
error fixtures represent the failure states an eventual adapter must handle.
No file contains an API key, cookie, email, account id, or raw browser
response.
