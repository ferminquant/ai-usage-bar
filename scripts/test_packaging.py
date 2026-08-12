"""Static contract checks for the Windows packaging lifecycle scripts.

The real install/upgrade/uninstall behavior runs in the Windows CI smoke job.
These checks keep the package contract visible to the fast cross-platform
quality job as well.
"""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
PACKAGING = ROOT / "packaging"


class PackagingContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.package = (PACKAGING / "package.ps1").read_text(encoding="utf-8")
        cls.install = (PACKAGING / "install.ps1").read_text(encoding="utf-8")
        cls.uninstall = (PACKAGING / "uninstall.ps1").read_text(encoding="utf-8")
        cls.smoke = (PACKAGING / "smoke-test.ps1").read_text(encoding="utf-8")
        cls.shell = (ROOT / "src" / "bin" / "shell.rs").read_text(encoding="utf-8")
        cls.workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )

    def test_package_records_payload_hashes_and_outer_checksum(self):
        for name in ("ai-usage-bar.exe", "ai-usage-bar-shell.exe", "install.ps1", "uninstall.ps1"):
            self.assertIn(name, self.package)
        self.assertIn("package-manifest.json", self.package)
        self.assertIn("checksums.sha256", self.package)
        self.assertIn("Get-FileHash", self.package)
        self.assertIn("Compress-Archive", self.package)
        self.assertIn("IsNullOrWhiteSpace($CertificateThumbprint)", self.package)

    def test_package_version_is_read_from_the_package_section(self):
        self.assertIn("[package]", self.package)
        self.assertIn("packageSection", self.package)
        self.assertIn("[package] section does not contain a version", self.package)

    def test_installer_is_user_scoped_and_transactional(self):
        self.assertIn("LocalApplicationData", self.install)
        self.assertIn("HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run", self.install)
        self.assertIn("__staging_", self.install)
        self.assertIn("__backup_", self.install)
        self.assertIn("__failed_", self.install)
        self.assertIn("TestFailureMode", self.install)
        self.assertIn("after-startup", self.install)
        self.assertIn("after-quarantine-blocked", self.install)
        self.assertIn("after-restore-blocked", self.install)
        self.assertIn("expectedChecksumPaths", self.install)
        self.assertIn("$originalError", self.install)
        self.assertIn("$installSucceeded", self.install)
        self.assertIn("APPDATA", self.install)
        self.assertIn("provider_data_is_outside_install_root", self.install)
        self.assertIn("$existingInstallation", self.install)
        self.assertIn("$preserveStartupDisabled", self.install)
        self.assertIn("preserved disabled preference", self.install)

    def test_shell_menu_exposes_startup_toggle(self):
        self.assertIn("MENU_TOGGLE_STARTUP", self.shell)
        self.assertIn("Run on Windows startup", self.shell)
        self.assertIn("set_auto_start_enabled", self.shell)

    def test_uninstaller_requires_marker_and_does_not_remove_config(self):
        self.assertIn("package-manifest.json", self.uninstall)
        self.assertIn("install-state.json", self.uninstall)
        self.assertIn('$manifest.product -ne "AI Usage Bar"', self.uninstall)
        self.assertIn('$state.schema_version -ne 1', self.uninstall)
        self.assertIn("Remove-Item -LiteralPath $InstallRoot -Recurse -Force", self.uninstall)
        self.assertNotIn("$env:APPDATA", self.uninstall)
        self.assertNotIn("Remove-Item -LiteralPath $env:APPDATA", self.uninstall)

    def test_smoke_covers_startup_upgrade_and_preservation(self):
        for marker in (
            "Start-Process",
            "SummaryPath",
            "shell.stderr.log",
            "config_path_is_read",
            "upgrade_preserves_config",
            "upgrade_preserves_disabled_startup",
            "uninstall_preserves_user_data",
            "startup_value_name",
            "rollback_recovery",
            "startup_rollback_recovery",
            "quarantine_recovery",
            "quarantine_failure_warning",
            "restore_failure_warning",
            "TestFailureMode",
        ):
            self.assertIn(marker, self.smoke)

    def test_windows_job_runs_packaging_smoke_and_uploads_evidence(self):
        self.assertIn("windows-package:", self.workflow)
        self.assertIn("packaging\\package.ps1", self.workflow)
        self.assertIn("packaging\\smoke-test.ps1", self.workflow)
        self.assertIn("name: windows-package", self.workflow)


if __name__ == "__main__":
    unittest.main()
