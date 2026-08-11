# CI runner strategy

## Decision

Use GitHub-hosted runners for this project's CI and release workflows. The
repository intentionally does not depend on a self-hosted runner.

## Why

GitHub-hosted runners provide a documented, repeatable baseline with less
administration and better isolation for pull requests from contributors. They
also make the project easier to build and audit after the repository is made
public.

The Windows packaging job runs on `windows-latest` because the desktop shell
and PowerShell installer need a native Windows environment. Linux jobs cover
the portable Rust tests, linting, security checks, and quality gates.

## Future options

If hosted-runner limits or a platform-specific test require a self-hosted
runner later, treat that as an explicit design change:

1. register a dedicated runner for this repository or an approved
   organization runner;
2. give it an unambiguous project-specific label;
3. keep credentials and unrelated workspaces off the machine;
4. accept code from forks only on isolated, least-privilege jobs; and
5. document the image, toolchain, cleanup, and monitoring requirements.

Do not broaden a workflow to `self-hosted` merely because a runner is
available. A self-hosted machine can expose credentials or unrelated files to
untrusted pull-request code, so the default remains GitHub-hosted execution.

## Safety requirements

- Never run untrusted fork code on a runner that can access provider data or
  long-lived credentials.
- Use a clean checkout and cleanup step for every job.
- Keep provider credentials out of CI; adapter tests use redacted fixtures and
  fake sessions.
- Pin setup actions where practical and record the runner image/toolchain.
- Validate Windows packaging separately; Linux runners cannot validate the
  desktop shell or installer.
