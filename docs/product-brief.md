# Product brief

## Working name

AI Usage Bar

## Problem

Hosted AI services expose usage limits in different places and with different
semantics. Codex, Kimi, Grok, Z.AI, and hosted Ollama services may use rolling
windows, weekly pools, credits, or spend. The user has to open several apps or
dashboards to understand what is available.

The goal is a calm, glanceable surface that reports the evidence each provider
actually exposes without inventing a common unit.

## Target user

A developer who actively uses multiple online AI subscriptions and wants a
small desktop indicator instead of several open dashboards.

## User stories

- As a user, I can see whether each provider is healthy, stale, unavailable,
  or not configured.
- As a user, I can see the active Codex/Kimi/Grok/Z.AI quota window and reset time
  without opening a browser.
- As a user, I can see each supported online provider independently without
  confusing unrelated quota, credit, and spend metrics.
- As a user, I can click the compact bar to inspect the source, timestamp,
  window semantics, and any error.
- As a user, I can disable a provider or hide a sensitive metric.
- As a maintainer, I can add a provider without changing the shell or
  normalization policy.

## Goals

### Near term

- Windows-first compact taskbar/tray experience.
- Local daemon with cached snapshots.
- Provider adapter boundary for Codex, Kimi, hosted Ollama, Grok, and Z.AI.
- Explicit freshness, confidence, and source labels.
- Offline fixture tests before live provider calls.

### Later

- macOS menu bar and Linux status-bar shells.
- Additional providers through the same adapter contract.
- Optional browser bridge for user-authorized dashboard-only metrics.
- Notifications for a user-configured threshold or reset event.

## Non-goals

- Automating prompts or provider usage.
- Circumventing rate limits, paywalls, or anti-bot controls.
- Uploading credentials or raw provider responses to a central service.
- Claiming exact quota data where the provider only exposes an estimate.
- A billing or invoice reconciliation product.

## Product decisions to validate

1. Is a Windows tray icon plus a lightweight popup sufficient, or is a true
   taskbar-integrated pill required?
2. Which Codex and Kimi surfaces can be accessed through stable,
   user-authorized interfaces?
3. Grok consumer remaining usage: **implement** via Grok Build CLI auth +
   cli-chat-proxy billing (not a browser bridge). See
   [grok-spike.md](spikes/grok-spike.md). Grok API remains a separate deferred
   surface.
4. Which Ollama Pro/cloud metrics are available programmatically, and which
   must remain dashboard-only?

## Success signals

The first release is useful when a user can understand provider state in under
five seconds, can tell when a value is stale or unavailable, and can add or
remove a provider without reinstalling the shell.
