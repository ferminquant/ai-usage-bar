# Windows packaging and release smoke tests

Issue [#16](https://github.com/ferminquant/ai-usage-bar/issues/16) uses a
portable, per-user ZIP package as the first Windows release format. It avoids
administrator access and keeps provider-owned credentials outside the
application directory.

## Package contents

`packaging/package.ps1` builds or packages the two Windows entrypoints:

- `ai-usage-bar-shell.exe` — the taskbar widget and refresh loop;
- `ai-usage-bar.exe` — the diagnostic command-line entrypoint;
- `install.ps1` and `uninstall.ps1` — user-scoped lifecycle scripts;
- `package-manifest.json` — version, commit, file hashes, install/config
  locations, and signing status;
- `checksums.sha256` plus a sidecar checksum for the ZIP itself.

The package is intentionally unsigned when no certificate is supplied. A
release build can pass a Windows SDK `signtool.exe` and certificate thumbprint
to `package.ps1`; the manifest then records Authenticode signing for both
executables. A release workflow without its protected signing secrets records
the same unsigned state in the manifest and release notes; this is an explicit
fallback, not an implicit claim that the package is signed.

Example from a Windows checkout:

```powershell
pwsh -File packaging/package.ps1 -OutputDirectory .\dist
```

To sign a release package, provide both signing parameters:

```powershell
pwsh -File packaging/package.ps1 `
  -OutputDirectory .\dist `
  -SignToolPath "C:\Program Files (x86)\Windows Kits\10\bin\x64\signtool.exe" `
  -CertificateThumbprint "<certificate-thumbprint>"
```

The thumbprint may be the certificate-store SHA-1 (40 hexadecimal characters)
or SHA-256 (64 hexadecimal characters); the package script selects the
corresponding `signtool` selector.

### Release signing policy

The tag-driven release job runs on a GitHub-hosted Windows runner in the
protected `release` environment. To enable signing, a maintainer must add both
of these as environment secrets on that environment:

- `WINDOWS_SIGNING_PFX_BASE64` — the base64-encoded PFX containing the code
  signing certificate and private key;
- `WINDOWS_SIGNING_PFX_PASSWORD` — the PFX password.

The workflow fails closed if only one secret is present. When both are present,
it imports the PFX into the runner's current-user certificate store, requires a
private key and the Code Signing EKU, rejects certificates that are not yet
valid or expire within seven days, locates the Windows SDK `signtool.exe`, and
passes the certificate thumbprint to `package.ps1`. Both Windows entrypoints
are then checked with `Get-AuthenticodeSignature`; a missing, invalid, or
unexpected signer blocks publication. The manifest records only the signing
mode and filenames, never the PFX, password, or certificate material. The
temporary PFX is deleted immediately after import and an `always()` cleanup
step removes the imported certificate from the runner store.

The certificate owner is responsible for issuance, rotation, revocation, and
renewal. Keep the `release` environment restricted to maintainers and require
environment approval for production tags. Do not copy the PFX into the
repository, a pull-request secret, a workflow artifact, or a self-hosted
runner. If the two environment secrets are absent, the workflow publishes an
explicitly unsigned package and says so in the release notes; no fake or
ephemeral certificate is generated.

## Install, upgrade, and uninstall behavior

The installer defaults to `%LOCALAPPDATA%\AI Usage Bar` and registers the
shell under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. It does
not require elevation. The user configuration remains at
`%APPDATA%\AI Usage Bar\config.json`; provider credentials and other
provider-owned data are never copied into or removed from the install root.

An upgrade stages the new package, stops only a running shell whose executable
path matches this installation, swaps the application directory, and removes
the temporary backup after success. If staging or registration fails, the
previous directory and startup value are restored. If a locked file prevents
cleanup of a failed swap, the partial directory is quarantined under a
`.__failed_<run-id>` suffix while the previous installation remains available
for recovery.

Uninstall removes the startup value only when it still points to this
installation, then removes the application directory. It deliberately leaves
the configuration and provider-owned data in place so a reinstall does not
sign users out or discard their settings.

The shell owns the current `RefreshService` in-process; there is no separate
daemon executable to install. The package therefore starts one shell process,
which contains the local refresh/cache lifecycle already covered by the Rust
tests.

The shell's right-click menu includes **Run on Windows startup**. It toggles
the same per-user `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
entry used by the installer and shows a checkmark when the current shell is
registered. Disabling it removes only an entry that points to this shell. A
conflicting entry is left untouched and reported as an error. Upgrades
preserve a disabled startup preference and repair a missing `Run` value when
startup was previously enabled; a first install enables startup by default
unless `-SkipStartup` is supplied. The preference is kept separately from the
command so an external cleanup cannot turn an enabled installation into a
silent opt-out.

For installs created before the durable preference existed, the installer has
no recorded "disabled" signal and can only infer the intent from the previous
state file: if it recorded startup as registered, an upgrade treats a missing
`Run` value as a lost registration and repairs it. This is what makes repair
possible, but it also means a user who disabled startup before the preference
existed and then upgraded could have startup re-enabled once. Such installs
self-correct immediately if the user opens the shell menu and toggles startup
off again; the preference is written from that point on.

## Verify and install a published release

Published releases remain manual. They may be Authenticode-signed or explicitly
unsigned, and the adjacent manifest and release notes report which mode was
used. The adjacent `.sha256` file verifies that the ZIP arrived intact, while
the package manifest and installer verify the contents after extraction. A
checksum does not prove who published a file, so download release assets only
from the repository's GitHub Release page and prefer a signed release when the
manifest reports `authenticode`.

After downloading the ZIP and its matching `.zip.sha256` sidecar into the same
directory, run this in PowerShell:

```powershell
$packagePath = (Get-Item .\ai-usage-bar-0.2.0-windows-x64.zip).FullName
$checksumPath = "$packagePath.sha256"
$expected = ((Get-Content -LiteralPath $checksumPath -Raw) -split '\s+')[0].ToLowerInvariant()
$actual = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) {
    throw "ZIP checksum mismatch. Expected $expected, got $actual."
}

$extractPath = Join-Path $env:TEMP "ai-usage-bar-0.2.0-release"
if (Test-Path -LiteralPath $extractPath) {
    Remove-Item -LiteralPath $extractPath -Recurse -Force
}
Expand-Archive -LiteralPath $packagePath -DestinationPath $extractPath
$installer = Join-Path $extractPath "install.ps1"
& pwsh -NoProfile -ExecutionPolicy Bypass -File $installer
if ($LASTEXITCODE -ne 0) {
    throw "The package installer failed with exit code $LASTEXITCODE."
}
```

The installer performs its own manifest and payload-checksum validation before
swapping the install directory. It preserves `%APPDATA%\AI Usage Bar\config.json`,
provider-owned data, and the existing per-user startup registration. It also
stages upgrades transactionally and restores the previous installation if a
swap or registration step fails. Close a development copy first if it is
running from a different directory; the installer only stops a shell whose
executable path matches the installed copy.

To verify the installed package after the command completes:

```powershell
$installed = Join-Path $env:LOCALAPPDATA "AI Usage Bar\package-manifest.json"
Get-Content -LiteralPath $installed -Raw | ConvertFrom-Json |
  Select-Object version, commit, signing
```

The package manifest records `signing.mode` as `signed` when the protected
certificate secrets are configured, or `unsigned` otherwise. The repository's
signing policy keeps certificate material in a maintainer-only release
environment; it must never be present in pull-request jobs, source, logs, or
release artifacts. When signing is enabled, both Windows entrypoints must be
signed and signature verification must pass before a release is published.
When signing is not enabled, the checksum-plus-manifest flow above remains the
supported update path; there is no silent updater or background executable
replacement.

## Clean-machine smoke test

`packaging/smoke-test.ps1` expands a package into an isolated temporary user
profile and verifies:

1. install and startup registration;
2. the installed CLI reads the isolated configuration path;
3. the shell process stays alive through its initial startup window;
4. failed swaps restore the previous version, including a locked-cleanup
   quarantine path and recovery-location warnings;
5. reinstall/upgrade installs a new manifest version while preserving a
   sentinel user configuration;
6. uninstall removes the package and startup value; and
7. configuration and provider-owned sentinel data remain unchanged.

The Windows CI workflow runs this smoke test on `windows-latest` and uploads
the ZIP, manifest, and checksums as a retained artifact. It uses no provider
credentials and does not exercise live provider calls as a release gate.

Known limitations: the first package is Windows x64 only and uses a portable
ZIP rather than an MSI. A signed public release requires a
maintainer-controlled Authenticode certificate; until it is configured,
release notes and the manifest make the unsigned state explicit. The package
does not install or alter any provider CLI or browser session.

## GitHub Release workflow

The repository's release workflow is deliberately tag-driven. To publish a
versioned package:

1. On a branch, update the package version in `Cargo.toml` and its matching
   `Cargo.lock` entry, and refresh the example package name in this document.
   Open a pull request with the bump: `main` is protected, so the change must
   pass the required status checks and be merged through review.
2. After the pull request merges, update local `main` and create an annotated
   tag with the same version, for example
   `git tag -a v0.2.0 -m "AI Usage Bar v0.2.0"`.
3. Push the tag with `git push origin v0.2.0`.

The workflow accepts only `vX.Y.Z` tags and refuses to publish when the tag
does not exactly match `Cargo.toml`. It builds both Windows entrypoints with
the locked dependency graph, runs the existing package script, and creates a
GitHub Release containing:

- the portable Windows x64 ZIP;
- the ZIP's adjacent SHA-256 checksum file;
- a copy of the package manifest; and
- a copy of the package payload checksum file.

The workflow uses a GitHub-hosted Windows runner and the repository-provided
`GITHUB_TOKEN`; it does not run for pull requests, read provider credentials,
copy browser cookies, or depend on a self-hosted runner. It signs and verifies
both entrypoints when the protected `release` environment is configured, and
otherwise publishes an explicitly unsigned package. Signing secrets must never
be exposed to pull-request workflows. CI runs `actionlint` against every
workflow file and the workflow contract tests before the Rust and packaging
jobs, so malformed workflow changes fail before they can reach a release tag.
