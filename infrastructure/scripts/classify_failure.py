#!/usr/bin/env python3
"""
AURA Failure Classification Script

Analyzes CI logs, crash dumps, and error output to classify failures
into the taxonomy system. Can be run as part of CI pipeline.

Usage:
    python classify_failure.py --log /path/to/log.txt
    python classify_failure.py --crash-dump /path/to/dump.txt
    python classify_failure.py --ci-output "raw CI output"
"""

import re
import sys
import argparse
from typing import Optional

# Failure patterns — order matters (most specific first)
FAILURE_PATTERNS = [
    # NDK/Compiler patterns
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
        "requires_all": False,
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
        "requires_all": False,
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
        "requires_all": False,
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
        "requires_all": False,
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
        "requires_all": False,
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
        "requires_all": False,
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
        "requires_all": False,
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
        "requires_all": False,
    },
    {
        "id": "F999",
        "name": "Unknown failure",
        "domain": "Unknown",
        "category": "UNKNOWN",
        "patterns": [],
        "requires_all": False,
    },
]


def classify_failure(text: str) -> dict:
    """Classify a failure from log text."""
    text_lower = text.lower()

    matches = []
    for pattern_def in FAILURE_PATTERNS:
        pattern_matches = 0
        for pattern in pattern_def["patterns"]:
            if re.search(pattern, text, re.IGNORECASE):
                pattern_matches += 1

        if pattern_matches > 0:
            matches.append(
                {
                    "id": pattern_def["id"],
                    "name": pattern_def["name"],
                    "domain": pattern_def["domain"],
                    "category": pattern_def["category"],
                    "match_count": pattern_matches,
                    "confidence": pattern_matches / len(pattern_def["patterns"])
                    if pattern_def["patterns"]
                    else 0,
                }
            )

    # Sort by match count descending
    matches.sort(key=lambda x: x["match_count"], reverse=True)

    if matches:
        best = matches[0]
        return {
            "classified": True,
            "id": best["id"],
            "name": best["name"],
            "domain": best["domain"],
            "category": best["category"],
            "confidence": best["confidence"],
            "all_matches": matches,
        }

    return {
        "classified": False,
        "id": "F999",
        "name": "Unknown failure",
        "domain": "Unknown",
        "category": "UNKNOWN",
        "confidence": 0.0,
        "all_matches": [],
    }


def main():
    parser = argparse.ArgumentParser(description="Classify AURA failures")
    parser.add_argument("--log", type=str, help="Path to log file")
    parser.add_argument("--crash-dump", type=str, help="Path to crash dump")
    parser.add_argument("--ci-output", type=str, help="Raw CI output string")
    args = parser.parse_args()

    text = ""
    if args.log:
        with open(args.log, "r", encoding="utf-8") as f:
            text = f.read()
    elif args.crash_dump:
        with open(args.crash_dump, "r", encoding="utf-8") as f:
            text = f.read()
    elif args.ci_output:
        text = args.ci_output
    else:
        # Read from stdin
        text = sys.stdin.read()

    result = classify_failure(text)

    print(f"\n{'=' * 60}")
    print(f"AURA FAILURE CLASSIFICATION REPORT")
    print(f"{'=' * 60}")
    print(f"Classification: {'SUCCESS' if result['classified'] else 'UNKNOWN'}")
    print(f"Failure ID:     {result['id']}")
    print(f"Name:           {result['name']}")
    print(f"Domain:         {result['domain']}")
    print(f"Category:       {result['category']}")
    print(f"Confidence:     {result['confidence']:.1%}")

    if result["all_matches"]:
        print(f"\nTop matches:")
        for m in result["all_matches"][:3]:
            print(f"  [{m['id']}] {m['name']} ({m['match_count']} pattern matches)")

    print(f"{'=' * 60}\n")

    # Exit with code based on classification
    sys.exit(0 if result["classified"] else 1)


if __name__ == "__main__":
    main()
