#!/usr/bin/env python3
"""Evaluate the checked-in coverage and mutation quality contract.

The script consumes machine-readable reports produced by cargo-llvm-cov and
cargo-mutants.  It deliberately keeps policy in ``quality/thresholds.json``
so a threshold change is reviewable next to its issue and expiry date.  The
full repository coverage remains visible in the artifact; the alpha gate is a
bounded policy-core scope whose exclusions are explicit and time limited.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
from pathlib import Path
from typing import Any


class QualityError(ValueError):
    """A malformed report or quality policy."""


def _number(value: Any, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise QualityError(f"{name} must be numeric")
    return float(value)


def _repo_relative(root: Path, filename: str) -> str:
    path = Path(filename)
    if path.is_absolute():
        path = Path(os.path.relpath(path, root))
    return Path(os.path.normpath(str(path))).as_posix()


def _line_counts(summary: dict[str, Any], label: str) -> dict[str, Any]:
    lines = summary.get("lines")
    if not isinstance(lines, dict):
        raise QualityError(f"{label} has no line summary")
    count = int(_number(lines.get("count"), f"{label}.count"))
    covered = int(_number(lines.get("covered"), f"{label}.covered"))
    if count < 0 or covered < 0 or covered > count:
        raise QualityError(f"{label} has invalid line counts")
    percent = (covered / count * 100.0) if count else 100.0
    return {"count": count, "covered": covered, "percent": round(percent, 4)}


def _validate_exclusions(
    exclusions: list[dict[str, Any]], today: dt.date
) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for exclusion in exclusions:
        if not isinstance(exclusion, dict):
            raise QualityError("coverage exclusions must be objects")
        path = exclusion.get("path")
        reason = exclusion.get("reason")
        issue = exclusion.get("issue")
        review_by = exclusion.get("review_by")
        if not isinstance(path, str) or not path:
            raise QualityError("every exclusion needs a path")
        if path in result:
            raise QualityError(f"duplicate exclusion: {path}")
        if not isinstance(reason, str) or not reason.strip():
            raise QualityError(f"{path}: exclusion needs a reason")
        if not isinstance(issue, int) or issue <= 0:
            raise QualityError(f"{path}: exclusion needs a positive issue number")
        if not isinstance(review_by, str):
            raise QualityError(f"{path}: exclusion needs review_by")
        try:
            review_date = dt.date.fromisoformat(review_by)
        except ValueError as exc:
            raise QualityError(f"{path}: review_by must be YYYY-MM-DD") from exc
        if review_date < today:
            raise QualityError(f"{path}: exclusion review date {review_by} has expired")
        result[path] = {
            "path": path,
            "reason": reason,
            "issue": issue,
            "review_by": review_by,
        }
    return result


def load_coverage_report(path: Path, root: Path, policy: dict[str, Any]) -> dict[str, Any]:
    report = json.loads(path.read_text(encoding="utf-8"))
    data = report.get("data")
    if not isinstance(data, list) or not data:
        raise QualityError("coverage report has no data entries")

    files: dict[str, dict[str, Any]] = {}
    for entry in data:
        for file_entry in entry.get("files", []):
            relative = _repo_relative(root, file_entry.get("filename", ""))
            if relative == "." or not relative:
                raise QualityError("coverage file has no filename")
            counts = _line_counts(file_entry.get("summary", {}), relative)
            previous = files.get(relative)
            if previous is not None and previous != counts:
                raise QualityError(f"coverage file appears with conflicting summaries: {relative}")
            files[relative] = counts

    totals = _line_counts(data[0].get("totals", {}), "coverage totals")
    coverage_policy = policy.get("coverage")
    if not isinstance(coverage_policy, dict):
        raise QualityError("policy has no coverage section")
    scope = coverage_policy.get("scope")
    if not isinstance(scope, list) or not all(isinstance(item, str) for item in scope):
        raise QualityError("coverage scope must be a list of paths")
    exclusions = _validate_exclusions(coverage_policy.get("excluded", []), dt.date.today())
    scope_set = set(scope)
    excluded_set = set(exclusions)
    report_set = set(files)
    missing = sorted(scope_set - report_set)
    if missing:
        raise QualityError(f"coverage report is missing scoped files: {', '.join(missing)}")
    unclassified = sorted(report_set - scope_set - excluded_set)
    if unclassified:
        raise QualityError(
            "coverage files must be scoped or explicitly excluded: "
            + ", ".join(unclassified)
        )
    unknown_exclusions = sorted(excluded_set - report_set)
    if unknown_exclusions:
        raise QualityError(
            "coverage exclusions are absent from the report: "
            + ", ".join(unknown_exclusions)
        )

    scoped_count = sum(files[path]["count"] for path in scope)
    scoped_covered = sum(files[path]["covered"] for path in scope)
    scoped = {
        "count": scoped_count,
        "covered": scoped_covered,
        "percent": round(scoped_covered / scoped_count * 100.0, 4)
        if scoped_count
        else 100.0,
    }
    scoped_threshold = _number(
        coverage_policy.get("scoped_line_threshold"),
        "coverage.scoped_line_threshold",
    )
    file_threshold = _number(
        coverage_policy.get("core_file_threshold"),
        "coverage.core_file_threshold",
    )
    gates = [
        {
            "name": "scoped line coverage",
            "threshold": scoped_threshold,
            "actual": scoped["percent"],
            "passed": scoped["percent"] >= scoped_threshold,
        }
    ]
    for path in scope:
        gates.append(
            {
                "name": f"core file line coverage: {path}",
                "threshold": file_threshold,
                "actual": files[path]["percent"],
                "passed": files[path]["percent"] >= file_threshold,
            }
        )
    return {
        "full": totals,
        "scoped": scoped,
        "scope_files": {path: files[path] for path in scope},
        "excluded_files": [
            exclusions[path] | {"coverage": files[path]} for path in sorted(exclusions)
        ],
        "all_files": files,
        "gates": gates,
    }


def load_mutation_report(path: Path, policy: dict[str, Any]) -> dict[str, Any]:
    report = json.loads(path.read_text(encoding="utf-8"))
    mutation_policy = policy.get("mutation")
    if not isinstance(mutation_policy, dict):
        raise QualityError("policy has no mutation section")
    values = {}
    for key in ("total_mutants", "caught", "missed", "timeout", "unviable"):
        value = report.get(key)
        if not isinstance(value, int) or value < 0:
            raise QualityError(f"mutation report has invalid {key}")
        values[key] = value
    if values["caught"] + values["missed"] + values["timeout"] + values["unviable"] != values[
        "total_mutants"
    ]:
        raise QualityError("mutation outcome counts do not add up to total_mutants")
    viable = values["caught"] + values["missed"] + values["timeout"]
    if viable == 0:
        raise QualityError("mutation report has no viable mutants")
    score = values["caught"] / viable * 100.0
    threshold = _number(
        mutation_policy.get("alpha_score_threshold"),
        "mutation.alpha_score_threshold",
    )
    scope_files = mutation_policy.get("scope_files")
    if not isinstance(scope_files, list) or not scope_files or not all(
        isinstance(item, str) and item for item in scope_files
    ):
        raise QualityError("mutation.scope_files must be a non-empty list of paths")
    regex_text = mutation_policy.get("regex")
    if not isinstance(regex_text, str) or not regex_text:
        raise QualityError("mutation.regex must be a non-empty string")
    try:
        mutation_regex = re.compile(regex_text)
    except re.error as exc:
        raise QualityError(f"mutation regex is invalid: {exc}") from exc

    outcomes = report.get("outcomes")
    if not isinstance(outcomes, list):
        raise QualityError("mutation report has no outcomes list")
    mutant_names: list[str] = []
    for outcome in outcomes:
        scenario = outcome.get("scenario") if isinstance(outcome, dict) else None
        mutant = scenario.get("Mutant") if isinstance(scenario, dict) else None
        if mutant is None:
            continue
        if not isinstance(mutant, dict):
            raise QualityError("mutation report has an invalid mutant scenario")
        name = mutant.get("name")
        file = mutant.get("file")
        if not isinstance(name, str) or not isinstance(file, str):
            raise QualityError("mutation report mutant is missing name or file")
        if file not in scope_files:
            raise QualityError(f"mutation report escaped scope: {file}")
        if not mutation_regex.search(name):
            raise QualityError(f"mutation report does not match mutation.regex: {name}")
        mutant_names.append(name)
    if len(mutant_names) != values["total_mutants"]:
        raise QualityError(
            "mutation report mutant count does not match total_mutants: "
            f"{len(mutant_names)} != {values['total_mutants']}"
        )
    gate = {
        "name": "policy-core mutation score",
        "threshold": threshold,
        "actual": round(score, 4),
        "passed": score >= threshold,
    }
    return {
        **values,
        "viable": viable,
        "score_percent": round(score, 4),
        "alpha_threshold": threshold,
        "pre_release_threshold": _number(
            mutation_policy.get("pre_release_score_threshold"),
            "mutation.pre_release_score_threshold",
        ),
        "scope_files": scope_files,
        "regex": regex_text,
        "gate": gate,
        "version": report.get("cargo_mutants_version"),
    }


def evaluate(coverage: dict[str, Any], mutation: dict[str, Any]) -> list[dict[str, Any]]:
    return coverage["gates"] + [mutation["gate"]]


def write_summary(
    path: Path,
    coverage: dict[str, Any],
    mutation: dict[str, Any],
    gates: list[dict[str, Any]],
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    summary = {
        "schema_version": 1,
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "coverage": {
            "full": coverage["full"],
            "scoped": coverage["scoped"],
            "scope_files": coverage["scope_files"],
            "excluded_files": coverage["excluded_files"],
        },
        "mutation": {
            key: mutation[key]
            for key in (
                "total_mutants",
                "caught",
                "missed",
                "timeout",
                "unviable",
                "viable",
                "score_percent",
                "alpha_threshold",
                "pre_release_threshold",
                "scope_files",
                "regex",
                "version",
            )
        },
        "gates": gates,
    }
    path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coverage", type=Path, required=True)
    parser.add_argument("--mutants", type=Path, required=True)
    parser.add_argument("--thresholds", type=Path, default=Path("quality/thresholds.json"))
    parser.add_argument(
        "--summary-out",
        type=Path,
        default=Path("quality-output/quality-summary.json"),
    )
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    root = args.root.resolve()
    try:
        policy = json.loads(args.thresholds.read_text(encoding="utf-8"))
        coverage = load_coverage_report(args.coverage, root, policy)
        mutation = load_mutation_report(args.mutants, policy)
        gates = evaluate(coverage, mutation)
    except (OSError, ValueError, json.JSONDecodeError, QualityError) as exc:
        print(f"Quality evidence is invalid: {exc}", file=sys.stderr)
        return 1

    write_summary(args.summary_out, coverage, mutation, gates)
    failed = [gate for gate in gates if not gate["passed"]]
    for gate in gates:
        status = "PASS" if gate["passed"] else "FAIL"
        print(
            f"{status}: {gate['name']} "
            f"{gate['actual']:.2f}% (threshold {gate['threshold']:.2f}%)"
        )
    print(f"Quality summary written to {args.summary_out}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
