# Self-hosted runner strategy

## What was observed in Budget

On 2026-07-31, ferminquant/budget exposed eight online Linux x64 runners.
They carry repository/workload-specific labels such as budget,
aws-workspace, and budget-u3m-*. Its workflow currently uses
runs-on: self-hosted.

## Can the same runners be reused?

Yes at the machine level, but not automatically at the repository level.
A self-hosted runner registration is scoped to a repository, organization, or
enterprise. A runner attached directly to ferminquant/budget will not
silently become eligible for ferminquant/ai-usage-bar.

The practical options are:

1. **GitHub-hosted runners for the first CI.** Lowest setup and isolation
   burden; use this for docs-only and early deterministic tests.
2. **Register another runner instance on the same host.** The existing Budget
   machines can run a separate runner registration for this repository. Give
   it an ai-usage-bar label and use an explicit selector such as
   runs-on: [self-hosted, Linux, X64, ai-usage-bar].
3. **Organization-level runner sharing.** If the projects move under a
   GitHub organization, an organization runner can be allowed for selected
   repositories. This is the cleanest shared-pool model, but it requires
   organization administration and a review of cross-repository isolation.

Do not rely on the broad self-hosted label for this project. The Budget
labels express assumptions about its workspace and dependencies that may not
hold for a desktop-widget build.

## Safety requirements

- Never run untrusted fork code on a runner that can access Budget data or
  long-lived credentials.
- Use a clean checkout and cleanup step for every job.
- Keep provider credentials out of CI; adapter tests use redacted fixtures and
  fake sessions.
- Pin setup actions and record the runner image/toolchain.
- Add a health check for Windows packaging separately; the existing Linux
  runners cannot validate the first desktop shell.

The runner decision is intentionally tracked as a GitHub issue so it can be
made after the first test/runtime measurements rather than assumed.
