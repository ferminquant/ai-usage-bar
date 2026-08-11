"""Static checks for the tag-driven GitHub Release workflow."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ReleaseWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )

    def test_release_is_tag_driven_and_writes_only_release_contents(self):
        self.assertIn("tags:", self.workflow)
        self.assertIn('"v[0-9]*.[0-9]*.[0-9]*"', self.workflow)
        self.assertIn("contents: write", self.workflow)
        self.assertIn("GITHUB_REF_TYPE", self.workflow)
        self.assertNotIn("pull_request:", self.workflow)
        self.assertNotIn("self-hosted", self.workflow)

    def test_release_validates_tag_against_cargo_version(self):
        self.assertIn("Cargo.toml", self.workflow)
        self.assertIn("[package]", self.workflow)
        self.assertIn("packageSection", self.workflow)
        self.assertIn("does not match Cargo.toml version", self.workflow)
        self.assertIn("RELEASE_VERSION", self.workflow)

    def test_release_builds_and_packages_the_windows_entrypoints(self):
        self.assertIn("runs-on: windows-latest", self.workflow)
        self.assertIn(
            "cargo build --release --locked --bin ai-usage-bar --bin ai-usage-bar-shell",
            self.workflow,
        )
        self.assertIn("packaging\\package.ps1", self.workflow)
        self.assertIn("$packageArguments = @{", self.workflow)
        self.assertIn("SkipBuild = $true", self.workflow)
        self.assertIn("@packageArguments", self.workflow)
        self.assertNotIn('"-SkipBuild"', self.workflow)
        self.assertIn("if (-not $?)", self.workflow)
        self.assertNotIn("Packaging failed with exit code $LASTEXITCODE", self.workflow)
        self.assertIn("package-manifest.json", self.workflow)
        self.assertIn("checksums.sha256", self.workflow)
        self.assertIn("manifestData.version", self.workflow)
        self.assertIn("manifestData.commit", self.workflow)

    def test_release_supports_guarded_signing_and_unsigned_fallback(self):
        self.assertIn("gh", self.workflow)
        self.assertIn('"release", "create"', self.workflow)
        self.assertIn("--verify-tag", self.workflow)
        self.assertIn("GH_TOKEN", self.workflow)
        self.assertIn(".sha256", self.workflow)
        self.assertIn("environment:", self.workflow)
        self.assertIn("name: release", self.workflow)
        self.assertIn("WINDOWS_SIGNING_PFX_BASE64", self.workflow)
        self.assertIn("WINDOWS_SIGNING_PFX_PASSWORD", self.workflow)
        self.assertIn("Import-PfxCertificate", self.workflow)
        self.assertIn('GetEnvironmentVariable("ProgramFiles(x86)")', self.workflow)
        self.assertIn('IsNullOrWhiteSpace($programFilesX86)', self.workflow)
        self.assertIn('Get-ChildItem -LiteralPath $_ -Directory', self.workflow)
        self.assertIn('Join-Path $_.FullName "x64\\signtool.exe"', self.workflow)
        self.assertIn("Get-AuthenticodeSignature", self.workflow)
        self.assertIn(
            'Authenticode verification failed for ${binaryName}:', self.workflow
        )
        self.assertIn(
            'unexpected Authenticode status for ${binaryName}:', self.workflow
        )
        self.assertNotIn(
            'Authenticode verification failed for $binaryName:', self.workflow
        )
        self.assertNotIn(
            'unexpected Authenticode status for $binaryName:', self.workflow
        )
        self.assertIn("Remove signing certificate from runner", self.workflow)
        self.assertIn("authenticode", self.workflow)
        self.assertIn("unsigned", self.workflow)
        self.assertNotIn("browser", self.workflow.lower())
        self.assertNotIn("cookie", self.workflow.lower())


if __name__ == "__main__":
    unittest.main()
