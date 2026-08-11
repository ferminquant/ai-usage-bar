# Contributing

Thanks for helping improve AI Usage Bar. Contributions are welcome through
issues and pull requests.

## Before you start

- Search the existing issues before opening a new one.
- For a new provider, record the supported usage source and its limitations
  before implementing an adapter.
- Never include API keys, access tokens, browser cookies, private keys, or raw
  authenticated responses in issues, fixtures, screenshots, or pull requests.

## Development workflow

1. Fork the repository or create a feature branch from `main`.
2. Keep changes focused on one issue or user-visible improvement.
3. Add or update deterministic tests and redacted fixtures with behavior
   changes.
4. Run the relevant checks locally:

   ```powershell
   cargo test --locked
   cargo clippy --locked --all-targets -- -D warnings
   python -m unittest discover --start-directory scripts --pattern "test_*.py"
   python scripts/check_secrets.py
   ```

5. Open a pull request that explains the user impact, validation performed,
   and any provider or platform limitation.

UI or packaging changes should include a screenshot or a short Windows smoke
test description when practical. Do not add a provider-specific shortcut to
the shell; keep provider behavior behind the adapter contract and cache.

## Pull request expectations

Pull requests should be small enough to review as one coherent change. CI
must pass before merge. Reviewers may ask for clearer evidence, additional
contract tests, or redaction of diagnostic output. By submitting a
contribution, you agree that it is provided under the repository's
MIT-or-Apache-2.0 licensing terms.
