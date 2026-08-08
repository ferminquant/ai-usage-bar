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
        self.assertIn('"v*.*.*"', self.workflow)
        self.assertIn("contents: write", self.workflow)
        self.assertIn("GITHUB_REF_TYPE", self.workflow)
        self.assertNotIn("pull_request:", self.workflow)
        self.assertNotIn("self-hosted", self.workflow)

    def test_release_validates_tag_against_cargo_version(self):
        self.assertIn("Cargo.toml", self.workflow)
        self.assertIn("does not match Cargo.toml version", self.workflow)
        self.assertIn("RELEASE_VERSION", self.workflow)

    def test_release_builds_and_packages_the_windows_entrypoints(self):
        self.assertIn("runs-on: windows-latest", self.workflow)
        self.assertIn(
            "cargo build --release --locked --bin ai-usage-bar --bin ai-usage-bar-shell",
            self.workflow,
        )
        self.assertIn("packaging\\package.ps1", self.workflow)
        self.assertIn("-SkipBuild", self.workflow)
        self.assertIn("if (-not $?)", self.workflow)
        self.assertNotIn("Packaging failed with exit code $LASTEXITCODE", self.workflow)
        self.assertIn("package-manifest.json", self.workflow)
        self.assertIn("checksums.sha256", self.workflow)
        self.assertIn("manifestData.version", self.workflow)
        self.assertIn("manifestData.commit", self.workflow)

    def test_release_publishes_verifiable_assets_without_signing_secrets(self):
        self.assertIn("gh", self.workflow)
        self.assertIn('"release", "create"', self.workflow)
        self.assertIn("--verify-tag", self.workflow)
        self.assertIn("GH_TOKEN", self.workflow)
        self.assertIn(".sha256", self.workflow)
        self.assertIn("unsigned", self.workflow)
        self.assertNotIn("CERTIFICATE_THUMBPRINT", self.workflow)
        self.assertNotIn("browser", self.workflow.lower())
        self.assertNotIn("cookie", self.workflow.lower())


if __name__ == "__main__":
    unittest.main()
