# Proposed architecture

## Boundary diagram

~~~text
Provider surfaces
  ├─ Codex app-server / local auth
  ├─ Kimi CLI or Console
  ├─ Ollama Pro/cloud supported surface
  ├─ Z.AI GLM Coding Plan monitor endpoint
  └─ Grok supported usage surface / optional browser bridge
             │
             ▼
      Provider adapters
             │  normalized UsageSnapshot[]
             ▼
       Local usage daemon
        ├─ scheduler and concurrency limits
        ├─ cache and freshness policy
        ├─ redaction and secure logging
        └─ provider registry/configuration
             │
             ├─ compact taskbar/tray view
             ├─ detailed provider view
             └─ diagnostics/export for local troubleshooting
~~~

## Core contracts

### Provider adapter

An adapter is responsible for:

- discovering whether the provider is configured;
- reading only the minimum data needed;
- converting the provider response into normalized snapshots;
- reporting source, observed time, reset time, and confidence;
- returning typed, redacted errors;
- never writing credentials or raw responses to the shared cache.

An adapter is not responsible for rendering UI or deciding how different
provider percentages should be combined.

### Usage snapshot

The authoritative contract is
[snapshot-contract.md](snapshot-contract.md), which refines the sketch
below with explicit state/value semantics, per-provider notes, fixture
requirements, and the unit/property/invariant test cases. Where the two
disagree, the contract file is authoritative.

The proposed snapshot shape is conceptually:

~~~text
provider
account_id (stable local identifier, never a secret)
metric_kind: quota | credits | spend | tokens | requests | health
window_kind: rolling | daily | weekly | monthly | session | none
used
remaining
limit
unit
resets_at
observed_at
source: api | cli | local_api | browser | fixture | system
freshness: live | cached | stale | unavailable | not_configured | not_applicable
confidence: exact | reported_estimate | inferred | unknown
error (optional, redacted, object with code and message)
~~~

Values may be absent. "Not reported" is different from zero, unlimited, and
unavailable.

### Cache

The cache is keyed by provider, account, metric, and window. A newer
observation must not be overwritten by an older concurrent response. Cached
data carries its original observed time and cannot be rendered as live merely
because the UI refreshed.

The default policy should favor a useful stale value with a visible stale
badge over a blank screen, while allowing the user to force a refresh.

## Shell boundary

The shell consumes normalized snapshots and state labels. It should not know
how a provider authenticates or whether the source was a CLI, API, or browser
bridge.

The first shell is Windows-first. A platform-neutral daemon keeps the future
macOS and Linux shells from duplicating provider logic.

## Security boundary

- Prefer existing local login sessions and OS credential stores.
- Keep tokens and cookies out of logs, issue attachments, and test fixtures.
- Redact provider payloads before diagnostics are persisted.
- Do not run arbitrary provider page JavaScript outside an explicit,
  user-authorized browser bridge.
- Treat provider responses as untrusted input: validate units, ranges,
  timestamps, and text lengths.

## Failure model

Every provider refresh should end in one of these states:

- live: a fresh successful observation;
- cached: a previously successful observation within the cache policy;
- stale: the last known observation is older than the policy allows;
- unavailable: configured, but the provider call failed;
- not configured: the user has not enabled or authenticated the provider;
- not applicable: the provider does not expose that metric.

The UI must preserve the distinction. A provider error must never delete the
last good snapshot or turn an unknown value into zero.
