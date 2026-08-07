# Security boundaries

The usage bar is a local, read-only monitor. Provider adapters use the
provider's existing local login/session and return typed `UsageSnapshot` data;
they do not upload credentials or authenticated responses to a project
service.

## Diagnostics and redaction

- Adapter errors are reduced to stable error codes at provider boundaries.
  Raw subprocess output, parser text, response bodies, paths, headers, and
  command arguments are not retained.
- The daemon applies a second boundary before an error can enter a snapshot:
  it drops the human-readable message and keeps only the stable error code.
  The view model applies pattern redaction before tooltip/detail/clipboard
  text is rendered, which protects direct diagnostic/test inputs as well.
- Account identifiers shown to users are stable safe identifiers. Email
  addresses, paths, and other unsafe values are replaced with a stable local
  hash. This is display-level pseudonymization for correlation, not
  cryptographic anonymity; it must never be treated as a credential or secret.
- `scripts/check_secrets.py` scans tracked text files for high-confidence
  private-key, provider-key, GitHub-token, JWT, bearer, authorization, and
  named-secret patterns. It reports only path, line, and rule name; secret
  contents are never printed.

The scanner is a guardrail, not proof that arbitrary secrets cannot exist.
Fixtures and tests must use placeholders or short synthetic values, and live
provider credentials must never be required by CI.

## Browser hand-off

The Windows shell has an explicit, user-invoked browser hand-off for provider
pages that do not expose a supported reset/usage API. The bridge:

1. accepts only fixed HTTPS URLs compiled into the application;
2. opens them only from the matching context-menu action; and
3. passes no cookies, authorization headers, page content, or URL supplied by
   provider data.

It does not automate a browser, scrape authenticated HTML, or run page
JavaScript. Adding another destination requires a code review and an explicit
allowlist test.

## CI gates

The `security` GitHub Actions job runs on every pull request and push to
`main`:

~~~text
python scripts/check_secrets.py
cargo audit
~~~

The JSON audit result is copied into the GitHub Actions job summary so the
quality record includes the dependency result without publishing a repository
artifact containing local paths or credentials.

The dependency audit is performed against the tracked Cargo dependency
lockfile in CI. A high-severity advisory or high-confidence secret finding
fails the job; no personal account or provider credential is used.
