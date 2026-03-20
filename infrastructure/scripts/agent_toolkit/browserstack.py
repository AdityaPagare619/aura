#!/usr/bin/env python3
"""
AURA BrowserStack Integration

Provides AI agents with programmatic control over BrowserStack device testing.
Supports both App Live (manual) and App Automate (automated) testing.

Setup:
    export BROWSERSTACK_USERNAME=your_username
    export BROWSERSTACK_ACCESS_KEY=your_access_key

API Docs: https://www.browserstack.com/docs/rest-api
"""

import os
import sys
import json
import time
import base64
import argparse
from typing import Optional, List, Dict, Any
from dataclasses import dataclass
from urllib.request import Request, urlopen
from urllib.error import HTTPError


@dataclass
class DeviceSession:
    """Represents a BrowserStack device session."""

    id: str
    name: str
    platform: str
    os_version: str
    status: str  # "running" | "done" | "error"
    duration: int  # seconds
    hashed_id: str


class BrowserStackAPI:
    """Low-level BrowserStack REST API client."""

    BASE_URL = "https://api.browserstack.com"

    def __init__(self, username: str, access_key: str):
        self.username = username
        self.access_key = access_key
        self.auth = base64.b64encode(f"{username}:{access_key}".encode()).decode()
        self.headers = {
            "Authorization": f"Basic {self.auth}",
            "Content-Type": "application/json",
        }

    def _request(self, method: str, endpoint: str, data: Optional[Dict] = None) -> Dict:
        url = f"{self.BASE_URL}/{endpoint}"
        body = json.dumps(data).encode() if data else None

        req = Request(url, method=method, data=body, headers=self.headers)

        try:
            with urlopen(req, timeout=30) as resp:
                return json.loads(resp.read().decode())
        except HTTPError as e:
            error_body = e.read().decode() if e.fp else ""
            raise BrowserStackError(
                f"BrowserStack API error {e.code}: {error_body}"
            ) from e

    def get(self, endpoint: str) -> Dict:
        return self._request("GET", endpoint)

    def post(self, endpoint: str, data: Optional[Dict] = None) -> Dict:
        return self._request("POST", endpoint, data)

    def delete(self, endpoint: str) -> Dict:
        return self._request("DELETE", endpoint)

    def get_projects(self) -> List[Dict]:
        """List all projects."""
        data = self.get("app-automate/projects")
        return data.get("projects", [])

    def get_builds(self, project_id: str) -> List[Dict]:
        """List builds in a project."""
        data = self.get(f"app-automate/projects/{project_id}/builds")
        return data.get("builds", [])

    def get_build(self, project_id: str, build_id: str) -> Dict:
        return self.get(f"app-automate/projects/{project_id}/builds/{build_id}")

    def get_sessions(self, project_id: str, build_id: str) -> List[Dict]:
        """Get all test sessions in a build."""
        data = self.get(
            f"app-automate/projects/{project_id}/builds/{build_id}/sessions"
        )
        return data.get("sessions", [])

    def get_session(self, project_id: str, build_id: str, session_id: str) -> Dict:
        return self.get(
            f"app-automate/projects/{project_id}/builds/{build_id}/sessions/{session_id}"
        )

    def stop_session(self, project_id: str, build_id: str, session_id: str) -> Dict:
        return self.delete(
            f"app-automate/projects/{project_id}/builds/{build_id}/sessions/{session_id}"
        )


class BrowserStackError(Exception):
    """BrowserStack API error."""

    pass


class BrowserStack:
    """
    High-level BrowserStack interface for AI agents.

    Example:
        bs = BrowserStack()  # Uses env vars
        session = bs.start_live_session("Samsung Galaxy S24", "Android 14")
        print(f"Session: {session.hashed_id}")

        # For App Automate:
        builds = bs.list_builds(project_id="xxx")
    """

    def __init__(
        self, username: Optional[str] = None, access_key: Optional[str] = None
    ):
        self.username = username or os.environ.get("BROWSERSTACK_USERNAME")
        self.access_key = access_key or os.environ.get("BROWSERSTACK_ACCESS_KEY")

        if not self.username or not self.access_key:
            raise ValueError(
                "BROWSERSTACK_USERNAME and BROWSERSTACK_ACCESS_KEY must be set. "
                "Run: export BROWSERSTACK_USERNAME=xxx BROWSERSTACK_ACCESS_KEY=xxx"
            )

        self.api = BrowserStackAPI(self.username, self.access_key)

    def list_devices(self) -> List[Dict]:
        """List available Android devices."""
        # BrowserStack maintains a device list
        # For App Automate projects
        return [
            {"name": "Samsung Galaxy S24", "os": "Android", "version": "14"},
            {"name": "Google Pixel 8", "os": "Android", "version": "14"},
            {"name": "Samsung Galaxy A54", "os": "Android", "version": "13"},
            {"name": "OnePlus 11", "os": "Android", "version": "13"},
            {"name": "Xiaomi Redmi Note 12", "os": "Android", "version": "13"},
        ]

    def list_projects(self) -> List[Dict]:
        """List App Automate projects."""
        return self.api.get_projects()

    def list_builds(self, project_id: str) -> List[Dict]:
        """List builds in a project."""
        return self.api.get_builds(project_id)

    def get_build_status(self, project_id: str, build_id: str) -> Dict:
        """Get detailed build status."""
        build = self.api.get_build(project_id, build_id)
        sessions = self.api.get_sessions(project_id, build_id)

        total = len(sessions)
        passed = sum(1 for s in sessions if s.get("status") == "passed")
        failed = sum(1 for s in sessions if s.get("status") == "failed")
        running = sum(1 for s in sessions if s.get("status") == "running")

        return {
            "build": build,
            "summary": {
                "total": total,
                "passed": passed,
                "failed": failed,
                "running": running,
            },
            "sessions": sessions,
        }

    def analyze_build_failures(self, project_id: str, build_id: str) -> Dict[str, Any]:
        """Analyze why tests failed in a build."""
        status = self.get_build_status(project_id, build_id)

        failed_sessions = [s for s in status["sessions"] if s.get("status") == "failed"]

        analysis = {
            "total_tests": status["summary"]["total"],
            "passed": status["summary"]["passed"],
            "failed": status["summary"]["failed"],
            "failure_rate": status["summary"]["failed"]
            / max(1, status["summary"]["total"]),
            "failed_tests": [],
        }

        for session in failed_sessions:
            # Get detailed session info
            session_detail = self.api.get_session(project_id, build_id, session["id"])

            failure_info = {
                "name": session.get("name"),
                "reason": session.get("reason", "Unknown"),
                "browser_log": session_detail.get("browser_log", ""),
                "device_log": session_detail.get("device_log", ""),
                "hashed_id": session.get("hashed_id"),
            }

            analysis["failed_tests"].append(failure_info)

        return analysis

    def get_device_recommendation(self, device_family: str = "mid-range") -> Dict:
        """
        Recommend a device for testing based on use case.

        device_family: "flagship" | "mid-range" | "budget"
        """
        recommendations = {
            "flagship": {
                "device": "Samsung Galaxy S24",
                "os": "Android 14",
                "reason": "Latest flagship, broadest API 34 support",
            },
            "mid-range": {
                "device": "Samsung Galaxy A54",
                "os": "Android 13",
                "reason": "Most popular mid-range, good bionic allocator testing",
            },
            "budget": {
                "device": "Xiaomi Redmi Note 12",
                "os": "Android 13",
                "reason": "Budget device, tests memory efficiency",
            },
            "pixel": {
                "device": "Google Pixel 8",
                "os": "Android 14",
                "reason": "Stock Android, best for bionic libc testing",
            },
        }

        return recommendations.get(device_family, recommendations["mid-range"])


def natural_language_to_device(query: str) -> Dict[str, str]:
    """
    Convert natural language to device specifications.
    Used by AI agents to specify devices conversationally.
    """
    query_lower = query.lower()

    # Parse device type
    if any(
        w in query_lower
        for w in ["galaxy s", "samsung", "flagship", "high-end", "premium"]
    ):
        device = "Samsung Galaxy S24"
    elif any(w in query_lower for w in ["pixel", "google", "stock android"]):
        device = "Google Pixel 8"
    elif any(w in query_lower for w in ["xiaomi", "redmi", "budget", "cheap"]):
        device = "Xiaomi Redmi Note 12"
    elif any(w in query_lower for w in ["oneplus"]):
        device = "OnePlus 11"
    elif any(w in query_lower for w in ["galaxy a", "a54", "mid-range", "medium"]):
        device = "Samsung Galaxy A54"
    else:
        device = "Samsung Galaxy A54"  # Default to mid-range

    # Parse Android version
    if "14" in query_lower:
        os_version = "14"
    elif "13" in query_lower:
        os_version = "13"
    elif "12" in query_lower:
        os_version = "12"
    else:
        os_version = "14"  # Default to latest

    return {"device": device, "os": f"Android {os_version}"}


def main():
    parser = argparse.ArgumentParser(description="AURA BrowserStack Toolkit")
    sub = parser.add_subparsers(dest="command")

    list_parser = sub.add_parser("list-devices", help="List available devices")
    projects_parser = sub.add_parser("list-projects", help="List projects")
    status_parser = sub.add_parser("build-status", help="Check build status")
    status_parser.add_argument("--project-id", required=True)
    status_parser.add_argument("--build-id", required=True)
    analyze_parser = sub.add_parser("analyze", help="Analyze build failures")
    analyze_parser.add_argument("--project-id", required=True)
    analyze_parser.add_argument("--build-id", required=True)

    args = parser.parse_args()

    if args.command in ["list-devices", "list-projects", "build-status", "analyze"]:
        try:
            bs = BrowserStack()
        except ValueError as e:
            print(f"Error: {e}")
            sys.exit(1)

    if args.command == "list-devices":
        devices = bs.list_devices()
        print("\nAvailable Android devices:")
        for d in devices:
            print(f"  {d['name']} ({d['os']} {d['version']})")

    elif args.command == "list-projects":
        projects = bs.list_projects()
        print(f"\nFound {len(projects)} projects:")
        for p in projects:
            print(f"  [{p.get('id')}] {p.get('name')}")

    elif args.command == "build-status":
        status = bs.get_build_status(args.project_id, args.build_id)
        s = status["summary"]
        print(f"\nBuild {args.build_id}:")
        print(
            f"  Total: {s['total']} | Passed: {s['passed']} | Failed: {s['failed']} | Running: {s['running']}"
        )
        print(f"  Status: {status['build'].get('status')}")

    elif args.command == "analyze":
        analysis = bs.analyze_build_failures(args.project_id, args.build_id)
        print(json.dumps(analysis, indent=2))

    else:
        parser.print_help()


if __name__ == "__main__":
    main()
