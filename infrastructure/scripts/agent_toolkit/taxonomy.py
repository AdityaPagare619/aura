#!/usr/bin/env python3
"""
AURA Taxonomy Classifier

Classifies CI failures using the failure taxonomy system.
Used by CI pipeline and AI agents to identify root causes.
"""

import re
import json
from typing import Dict, Any, List, Optional


class TaxonomyClassifier:
    """Classifies failures against the AURA taxonomy."""

    def __init__(self):
        self.patterns = [
            {
                "id": "F001",
                "name": "SIGSEGV at startup with NDK r26b",
                "domain": "NdkCompiler",
                "category": "SIGSEGV",
                "patterns": [
                    r"SIGSEGV",
                    r"signal: 11",
                    r"Segmentation fault",
                    r"crash addr 0x[0-9a-f]+",
                    r"NDK r26b",
                ],
            },
            {
                "id": "F002",
                "name": "Bionic allocator OOM",
                "domain": "Memory",
                "category": "OOM",
                "patterns": [
                    r"out of memory",
                    r"OOM",
                    r"allocation failed",
                    r"bionic.*malloc.*NULL",
                    r"memory limit exceeded",
                ],
            },
            {
                "id": "F003",
                "name": "Termux permission denied",
                "domain": "Platform",
                "category": "PERMISSION_DENIED",
                "patterns": [
                    r"permission denied",
                    r"EACCES",
                    r"EPERM",
                    r"cannot execute",
                ],
            },
            {
                "id": "F004",
                "name": "Reflection schema mismatch",
                "domain": "Logic",
                "category": "SCHEMA_MISMATCH",
                "patterns": [
                    r"reflection.*schema.*mismatch",
                    r"invalid.*verdict.*format",
                    r"expected.*safe.*correct.*concerns",
                ],
            },
            {
                "id": "F005",
                "name": "Semantic similarity stub",
                "domain": "Logic",
                "category": "SCHEMA_MISMATCH",
                "patterns": [
                    r"semantic.*similarity.*0\.0",
                    r"always.*returns.*zero",
                ],
            },
            {
                "id": "F006",
                "name": "Ethics audit bypass",
                "domain": "Logic",
                "category": "ETHICS_BYPASS",
                "patterns": [
                    r"ethics.*bypass",
                    r"audit.*downgraded",
                    r"trust.*override.*ethics",
                ],
            },
            {
                "id": "F007",
                "name": "GDPR erasure incomplete",
                "domain": "Logic",
                "category": "SCHEMA_MISMATCH",
                "patterns": [
                    r"gdpr.*erasure.*incomplete",
                    r"right.*to.*erasure.*failed",
                    r"only.*user_profile.*deleted",
                ],
            },
            {
                "id": "F008",
                "name": "LLama.cpp crash",
                "domain": "Inference",
                "category": "LLAMA_CPP_CRASH",
                "patterns": [
                    r"llama.*crash",
                    r"model.*load.*failed",
                    r"llama_load_model.*error",
                ],
            },
        ]

    def classify(self, text: str) -> Dict[str, Any]:
        """Classify a failure from text."""
        matches = []

        for pattern_def in self.patterns:
            for pattern in pattern_def["patterns"]:
                if re.search(pattern, text, re.IGNORECASE):
                    matches.append(pattern_def)
                    break

        if matches:
            best = matches[0]
            return {
                "classified": True,
                "id": best["id"],
                "name": best["name"],
                "domain": best["domain"],
                "category": best["category"],
            }

        return {
            "classified": False,
            "id": "F999",
            "name": "Unknown failure",
            "domain": "Unknown",
            "category": "UNKNOWN",
        }

    def get_prevention_checklist(self, failure_id: str) -> List[str]:
        """Get prevention checklist for a failure ID."""
        checklists = {
            "F001": [
                "Check Cargo.toml for lto=true + panic=abort combination",
                "Run regression test: tests/regression/test_ndk_lto.rs",
                "Verify container uses NDK r26b specifically",
            ],
            "F002": [
                "Verify memory usage in container with --memory=512m",
                "Check for memory leaks using valgrind or heap profiling",
                "Consider custom allocator for bionic compatibility",
            ],
            "F003": [
                "Verify HOME environment variable is set",
                "Check Termux storage permissions: termux-setup-storage",
                "Verify binary has execute permission: chmod +x",
            ],
            "F004": [
                "Run schema validation tests",
                "Verify prompts.rs output matches grammar.rs GBNF",
                "Check reflection layer integration tests",
            ],
            "F005": [
                "Run semantic similarity unit tests",
                "Verify LCS algorithm produces correct scores",
                "Test with known similar/dissimilar string pairs",
            ],
            "F006": [
                "Run ethics bypass tests",
                "Verify Audit verdicts are non-bypassable",
                "Test at all trust levels (0.0 to 1.0)",
            ],
            "F007": [
                "Run GDPR integration test",
                "Verify all 5 memory tiers are cleared",
                "Test export_comprehensive() for all data types",
            ],
            "F008": [
                "Verify model file exists and is valid GGUF",
                "Check llama.cpp version compatibility",
                "Run model loading smoke test",
            ],
        }

        return checklists.get(failure_id, ["Unknown failure — investigate manually"])


def main():
    """CLI interface for taxonomy classifier."""
    import argparse
    import sys

    parser = argparse.ArgumentParser(description="AURA Taxonomy Classifier")
    sub = parser.add_subparsers(dest="command")

    classify_parser = sub.add_parser("classify", help="Classify failure text")
    classify_parser.add_argument("text", help="Failure text or log content")
    classify_parser.add_argument("--json", action="store_true", help="Output as JSON")

    list_parser = sub.add_parser("list", help="List all taxonomy entries")
    checklist_parser = sub.add_parser("checklist", help="Get prevention checklist")
    checklist_parser.add_argument("failure_id", help="Failure ID (e.g., F001)")

    args = parser.parse_args()

    classifier = TaxonomyClassifier()

    if args.command == "classify":
        result = classifier.classify(args.text)
        if args.json:
            print(json.dumps(result, indent=2))
        else:
            if result["classified"]:
                print(f"Classified: {result['id']} - {result['name']}")
                print(f"Domain: {result['domain']} | Category: {result['category']}")
            else:
                print(f"Unclassified: {result['id']} - {result['name']}")

    elif args.command == "list":
        print("AURA Failure Taxonomy:")
        for p in classifier.patterns:
            print(f"  {p['id']}: {p['name']} ({p['domain']})")
        print(f"  F999: Unknown failure")

    elif args.command == "checklist":
        checklist = classifier.get_prevention_checklist(args.failure_id)
        print(f"Prevention Checklist for {args.failure_id}:")
        for i, item in enumerate(checklist, 1):
            print(f"  {i}. {item}")

    else:
        parser.print_help()


if __name__ == "__main__":
    main()
