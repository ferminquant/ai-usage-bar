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
executables. Unsigned CI artifacts are validation artifacts, not publication
artifacts.

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

## Install, upgrade, and uninstall behavior

The installer defaults to `%LOCALAPPDATA%\AI Usage Bar` and registers the
shell under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. It does
not require elevation. The user configuration remains at
`%APPDATA%\AI Usage Bar\config.json`; provider credentials and other
provider-owned data are never copied into or removed from the install root.

An upgrade stages the new package, stops only a running shell whose executable
path matches this installation, swaps the application directory, and removes
the temporary backup after success. If staging or registration fails, the
previous directory and startup value are restored.

Uninstall removes the startup value only when it still points to this
installation, then removes the application directory. It deliberately leaves
the configuration and provider-owned data in place so a reinstall does not
sign users out or discard their settings.

The shell owns the current `RefreshService` in-process; there is no separate
daemon executable to install. The package therefore starts one shell process,
which contains the local refresh/cache lifecycle already covered by the Rust
tests.

## Clean-machine smoke test

`packaging/smoke-test.ps1` expands a package into an isolated temporary user
profile and verifies:

1. install and startup registration;
2. the shell process stays alive through its initial startup window;
3. reinstall/upgrade preserves a sentinel user configuration;
4. uninstall removes the package and startup value; and
5. configuration and provider-owned sentinel data remain unchanged.

The Windows CI workflow runs this smoke test on `windows-latest` and uploads
the ZIP, manifest, and checksums as a retained artifact. It uses no provider
credentials and does not exercise live provider calls as a release gate.

Known limitations: the first package is Windows x64 only, uses a portable ZIP
rather than an MSI, and requires a maintainer-controlled Authenticode
certificate before public release. The package does not install or alter any
provider CLI or browser session.
