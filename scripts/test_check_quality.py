#!/usr/bin/env python3
"""Small offline tests for the quality evidence parser and gates."""

from __future__ import annotations

import datetime as dt
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_quality  # noqa: E402


class QualityEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = {
            "coverage": {
                "scoped_line_threshold": 90,
                "core_file_threshold": 90,
                "scope": ["src/model.rs"],
                "excluded": [
                    {
                        "path": "src/bin/main.rs",
                        "reason": "entrypoint smoke test",
                        "issue": 13,
                        "review_by": (dt.date.today() + dt.timedelta(days=1)).isoformat(),
                    }
                ],
            },
            "mutation": {
                "alpha_score_threshold": 70,
                "pre_release_score_threshold": 80,
                "scope_files": ["src/model.rs"],
                "regex": "model",
                "excluded": [],
            },
        }

    def test_coverage_scope_is_aggregated_and_gated(self) -> None:
        report = {
            "data": [
                {
                    "files": [
                        {
                            "filename": "/repo/src/model.rs",
                            "summary": {"lines": {"count": 10, "covered": 10}},
                        },
                        {
                            "filename": "/repo/src/bin/main.rs",
                            "summary": {"lines": {"count": 2, "covered": 0}},
                        },
                    ],
                    "totals": {"lines": {"count": 12, "covered": 10}},
                }
            ]
        }
        import json
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "coverage.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            result = check_quality.load_coverage_report(path, Path("/repo"), self.policy)
        self.assertEqual(result["scoped"]["percent"], 100.0)
        self.assertTrue(result["gates"][0]["passed"])
        self.assertEqual(result["full"]["percent"], 83.3333)

    def test_mutation_score_excludes_unviable_mutants(self) -> None:
        import json
        import tempfile

        report = {
            "total_mutants": 5,
            "caught": 3,
            "missed": 1,
            "timeout": 0,
            "unviable": 1,
            "cargo_mutants_version": "test",
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "outcomes.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            result = check_quality.load_mutation_report(path, self.policy)
        self.assertEqual(result["viable"], 4)
        self.assertEqual(result["score_percent"], 75.0)
        self.assertTrue(result["gate"]["passed"])


if __name__ == "__main__":
    unittest.main()
