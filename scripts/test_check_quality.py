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
            },
        }

    def test_coverage_scope_is_aggregated_and_gated(self) -> None:
        import copy
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
            root = Path(directory) / "repo"
            (root / "src").mkdir(parents=True)
            (root / "src" / "lib.rs").write_text("pub mod model;\n", encoding="utf-8")
            for file_entry in report["data"][0]["files"]:
                relative = Path(file_entry["filename"]).relative_to("/repo")
                file_entry["filename"] = str(root / relative)
            policy = copy.deepcopy(self.policy)
            policy["coverage"]["excluded"].append(
                {
                    "path": "src/lib.rs",
                    "reason": "no instrumented executable lines",
                    "issue": 13,
                    "review_by": (dt.date.today() + dt.timedelta(days=1)).isoformat(),
                }
            )
            path = Path(directory) / "coverage.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            result = check_quality.load_coverage_report(path, root, policy)
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
            "success": 0,
            "cargo_mutants_version": "test",
            "outcomes": [
                {"scenario": {"Baseline": {}}, "summary": "Success"},
                *[
                    {
                        "scenario": {
                            "Mutant": {
                                "name": "src/model.rs:1: model mutation",
                                "file": "src/model.rs",
                            }
                        },
                        "summary": "CaughtMutant",
                    }
                    for _ in range(5)
                ],
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "outcomes.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            result = check_quality.load_mutation_report(path, self.policy)
        self.assertEqual(result["viable"], 4)
        self.assertEqual(result["score_percent"], 75.0)
        self.assertTrue(result["gate"]["passed"])

    def test_mutation_successes_are_viable(self) -> None:
        import json
        import tempfile

        report = {
            "total_mutants": 2,
            "caught": 1,
            "missed": 0,
            "timeout": 0,
            "unviable": 0,
            "success": 1,
            "outcomes": [
                {"scenario": {"Baseline": {}}, "summary": "Success"},
                *[
                    {
                        "scenario": {
                            "Mutant": {
                                "name": "src/model.rs:1: model mutation",
                                "file": "src/model.rs",
                            }
                        },
                        "summary": "CaughtMutant",
                    },
                    {
                        "scenario": {
                            "Mutant": {
                                "name": "src/model.rs:2: model survivor",
                                "file": "src/model.rs",
                            }
                        },
                        "summary": "Success",
                    },
                ],
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "outcomes.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            result = check_quality.load_mutation_report(path, self.policy)
        self.assertEqual(result["viable"], 2)
        self.assertEqual(result["score_percent"], 50.0)
        self.assertFalse(result["gate"]["passed"])

    def test_coverage_rejects_unclassified_files(self) -> None:
        import json
        import tempfile

        report = {
            "data": [
                {
                    "files": [
                        {
                            "filename": "/repo/src/model.rs",
                            "summary": {"lines": {"count": 10, "covered": 10}},
                        },
                        {
                            "filename": "/repo/src/new.rs",
                            "summary": {"lines": {"count": 1, "covered": 1}},
                        },
                        {
                            "filename": "/repo/src/bin/main.rs",
                            "summary": {"lines": {"count": 2, "covered": 0}},
                        },
                    ],
                    "totals": {"lines": {"count": 13, "covered": 11}},
                }
            ]
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "coverage.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            with self.assertRaises(check_quality.QualityError):
                check_quality.load_coverage_report(path, Path("/repo"), self.policy)

    def test_expired_exclusion_is_rejected(self) -> None:
        import json
        import tempfile

        policy = json.loads(json.dumps(self.policy))
        policy["coverage"]["excluded"][0]["review_by"] = "2020-01-01"
        report = {
            "data": [
                {
                    "files": [
                        {
                            "filename": "/repo/src/model.rs",
                            "summary": {"lines": {"count": 1, "covered": 1}},
                        },
                        {
                            "filename": "/repo/src/bin/main.rs",
                            "summary": {"lines": {"count": 1, "covered": 0}},
                        },
                    ],
                    "totals": {"lines": {"count": 2, "covered": 1}},
                }
            ]
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "coverage.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            with self.assertRaises(check_quality.QualityError):
                check_quality.load_coverage_report(path, Path("/repo"), policy)

    def test_malformed_policy_is_rejected_without_assuming_report_shape(self) -> None:
        import json
        import tempfile

        report = {
            "total_mutants": 0,
            "caught": 0,
            "missed": 0,
            "timeout": 0,
            "unviable": 0,
            "success": 0,
            "outcomes": [],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "outcomes.json"
            path.write_text(json.dumps(report), encoding="utf-8")
            with self.assertRaises(check_quality.QualityError):
                check_quality.load_mutation_report(path, self.policy)


if __name__ == "__main__":
    unittest.main()
