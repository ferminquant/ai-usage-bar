# Security policy

AI Usage Bar is a local Windows application that reads provider-owned CLI
sessions and usage surfaces. It is designed not to send credentials or usage
responses to a project server.

## Supported versions

Security fixes target the latest release in the `0.1.x` series. Older release
artifacts may not receive fixes.

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Use GitHub's
private vulnerability reporting for this repository (`Security` → `Report a
vulnerability`). If private reporting is unavailable, contact the repository
owner through their GitHub profile and include only the minimum reproducible
details needed to investigate.

Do not send access tokens, API keys, browser cookies, private keys, or raw
authenticated provider responses. Redact them before sharing logs or files.

Reports should include the affected release or commit, operating system,
reproduction steps, impact, and any suggested mitigation. We will acknowledge
valid reports, coordinate a fix, and document the affected release when a
public advisory is appropriate.

## Privacy boundary

Provider credentials remain in provider-owned local session stores. Diagnostic
output and fixtures must be redacted, and provider dashboards or authenticated
responses must not be committed to the repository.
