"""Static checks for the repository's workflow validation and release docs."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class WorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        workflows = ROOT / ".github" / "workflows"
        cls.ci = (workflows / "ci.yml").read_text(encoding="utf-8")
        cls.release = (workflows / "release.yml").read_text(encoding="utf-8")
        cls.packaging_docs = (ROOT / "docs" / "packaging.md").read_text(
            encoding="utf-8"
        )
        cls.readme = (ROOT / "README.md").read_text(encoding="utf-8")

    def test_ci_has_an_explicit_actionlint_gate(self):
        self.assertIn("workflow-validation:", self.ci)
        self.assertIn("uses: raven-actions/actionlint@v2", self.ci)
        self.assertIn("scripts.test_workflow_contract", self.ci)

    def test_release_workflow_stays_on_github_hosted_runner(self):
        self.assertIn("runs-on: windows-latest", self.release)
        self.assertNotIn("self-hosted", self.ci.lower())
        self.assertNotIn("self-hosted", self.release.lower())

    def test_manual_upgrade_docs_verify_checksum_before_install(self):
        self.assertIn("Verify and install a published release", self.packaging_docs)
        self.assertIn("Get-FileHash", self.packaging_docs)
        self.assertIn("Expand-Archive", self.packaging_docs)
        self.assertIn("install.ps1", self.packaging_docs)
        self.assertIn("unsigned", self.packaging_docs.lower())
        self.assertIn("docs/packaging.md#verify-and-install-a-published-release", self.readme)


if __name__ == "__main__":
    unittest.main()
