#!/usr/bin/env python3
"""Compare analyzer-lsp output against koncur expected-output.yaml.

Usage:
    python compare-koncur.py <actual> <expected>
    python compare-koncur.py hack/output.yaml /path/to/koncur/tests/nerd-dinner/expected-output.yaml
"""

import sys
import yaml
from pathlib import Path
from urllib.parse import urlparse


def normalize_uri(uri: str) -> str:
    """Strip scheme and leading path prefixes, return relative path from mvc4/."""
    parsed = urlparse(uri)
    path = parsed.path
    # Find mvc4/ and return everything from there
    idx = path.find("mvc4/")
    if idx != -1:
        return path[idx:]
    # Fallback: return filename
    return path.rsplit("/", 1)[-1] if "/" in path else path


def load_rulesets(path: str) -> list:
    with open(path) as f:
        return yaml.safe_load(f)


def extract_rules(rulesets: list) -> dict:
    """Extract {rule_id: {section, incidents}} from rulesets."""
    rules = {}
    for rs in rulesets:
        for section in ("violations", "insights"):
            section_data = rs.get(section, {}) or {}
            for rule_id, rule_data in section_data.items():
                incidents = rule_data.get("incidents", [])
                rules[rule_id] = {
                    "section": section,
                    "count": len(incidents),
                    "incidents": incidents,
                }
        for rule_id in rs.get("unmatched", []) or []:
            if rule_id not in rules:
                rules[rule_id] = {"section": "unmatched", "count": 0, "incidents": []}
    return rules


def compare_incidents(actual_incs: list, expected_incs: list) -> dict:
    """Compare incident lists, return match stats."""
    actual_keys = set()
    for inc in actual_incs:
        uri = normalize_uri(inc.get("uri", ""))
        line = inc.get("lineNumber", -1)
        actual_keys.add((uri, line))

    expected_keys = set()
    for inc in expected_incs:
        uri = normalize_uri(inc.get("uri", ""))
        line = inc.get("lineNumber", -1)
        expected_keys.add((uri, line))

    matched = actual_keys & expected_keys
    missing = expected_keys - actual_keys
    extra = actual_keys - expected_keys

    return {
        "matched": len(matched),
        "missing": sorted(missing),
        "extra": sorted(extra),
    }


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)

    actual_path = sys.argv[1]
    expected_path = sys.argv[2]

    if not Path(actual_path).exists():
        print(f"ERROR: actual file not found: {actual_path}")
        sys.exit(1)
    if not Path(expected_path).exists():
        print(f"ERROR: expected file not found: {expected_path}")
        sys.exit(1)

    actual_rulesets = load_rulesets(actual_path)
    expected_rulesets = load_rulesets(expected_path)

    actual_rules = extract_rules(actual_rulesets)
    expected_rules = extract_rules(expected_rulesets)

    all_rule_ids = sorted(set(actual_rules) | set(expected_rules))

    matched_rules = 0
    mismatched_rules = []
    missing_rules = []
    extra_rules = []
    total_incidents_matched = 0
    total_incidents_missing = 0
    total_incidents_extra = 0

    for rule_id in all_rule_ids:
        actual = actual_rules.get(rule_id)
        expected = expected_rules.get(rule_id)

        if actual is None:
            missing_rules.append(rule_id)
            continue
        if expected is None:
            extra_rules.append(rule_id)
            continue

        if actual["count"] == 0 and expected["count"] == 0:
            matched_rules += 1
            continue

        if actual["count"] == 0 and expected["count"] > 0:
            mismatched_rules.append(
                f"  {rule_id}: expected {expected['count']} incidents, got 0"
            )
            total_incidents_missing += expected["count"]
            continue

        if expected["count"] == 0 and actual["count"] > 0:
            mismatched_rules.append(
                f"  {rule_id}: expected 0 incidents (unmatched), got {actual['count']}"
            )
            total_incidents_extra += actual["count"]
            continue

        cmp = compare_incidents(actual["incidents"], expected["incidents"])
        total_incidents_matched += cmp["matched"]
        total_incidents_missing += len(cmp["missing"])
        total_incidents_extra += len(cmp["extra"])

        if cmp["missing"] or cmp["extra"]:
            mismatched_rules.append(
                f"  {rule_id}: {cmp['matched']} matched, "
                f"{len(cmp['missing'])} missing, {len(cmp['extra'])} extra "
                f"(actual={actual['count']}, expected={expected['count']})"
            )
        else:
            matched_rules += 1

    # --- Print report ---
    print("=" * 60)
    print("Koncur Comparison Report")
    print("=" * 60)
    print(f"Rules in actual:   {len(actual_rules)}")
    print(f"Rules in expected: {len(expected_rules)}")
    print()
    print(f"Fully matched rules: {matched_rules}")

    if missing_rules:
        print(f"\nMissing rules ({len(missing_rules)}) - in expected but not actual:")
        for r in missing_rules:
            exp = expected_rules[r]
            print(f"  {r} ({exp['section']}, {exp['count']} incidents)")

    if extra_rules:
        print(f"\nExtra rules ({len(extra_rules)}) - in actual but not expected:")
        for r in extra_rules:
            act = actual_rules[r]
            print(f"  {r} ({act['section']}, {act['count']} incidents)")

    if mismatched_rules:
        print(f"\nMismatched rules ({len(mismatched_rules)}):")
        for line in mismatched_rules:
            print(line)

    print()
    print(f"Incidents: {total_incidents_matched} matched, "
          f"{total_incidents_missing} missing, {total_incidents_extra} extra")
    print("=" * 60)

    if missing_rules or mismatched_rules:
        sys.exit(1)
    print("PASS")


if __name__ == "__main__":
    main()
