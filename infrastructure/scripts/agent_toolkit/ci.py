#!/usr/bin/env python3
"""
AURA CI Toolkit — GitHub Actions API wrapper

Provides AI agents with programmatic control over GitHub Actions CI/CD.
Agents can trigger builds, read logs, check status, and analyze failures.

Setup:
    export GITHUB_TOKEN=ghp_your_token_here
    # or
    python ci.py --token ghp_xxx trigger --workflow aura-android-validate.yml

API Docs: https://docs.github.com/en/rest/actions
"""

import os
import sys
import json
import time
import argparse
from typing import Optional, List, Dict, Any
from dataclasses import dataclass
from datetime import datetime
from urllib.request import Request, urlopen
from urllib.error import HTTPError, URLError


@dataclass
class WorkflowRun:
    """Represents a single workflow run."""

    id: int
    name: str
    status: str  # "queued" | "in_progress" | "completed"
    conclusion: Optional[str]  # "success" | "failure" | "cancelled" | None
    head_sha: str
    branch: str
    created_at: str
    updated_at: str
    html_url: str
    logs_url: str


@dataclass
class JobInfo:
    """Represents a job within a workflow run."""

    id: int
    name: str
    status: str
    conclusion: Optional[str]
    started_at: Optional[str]
    completed_at: Optional[str]


class GitHubAPI:
    """Low-level GitHub API client."""

    BASE_URL = "https://api.github.com"

    def __init__(self, token: str, owner: str = "AdityaPagare619", repo: str = "aura"):
        self.token = token
        self.owner = owner
        self.repo = repo
        self.headers = {
            "Authorization": f"token {token}",
            "Accept": "application/vnd.github.v3+json",
            "X-GitHub-Api-Version": "2022-11-28",
        }

    def _request(self, method: str, endpoint: str, data: Optional[Dict] = None) -> Dict:
        """Make an authenticated API request."""
        url = f"{self.BASE_URL}/{endpoint}"
        body = json.dumps(data).encode() if data else None

        req = Request(url, method=method, data=body, headers=self.headers)

        try:
            with urlopen(req, timeout=30) as resp:
                return json.loads(resp.read().decode())
        except HTTPError as e:
            error_body = e.read().decode() if e.fp else ""
            raise GitHubAPIError(f"GitHub API error {e.code}: {error_body}") from e
        except URLError as e:
            raise GitHubAPIError(f"Network error: {e.reason}") from e

    def get(self, endpoint: str) -> Dict:
        return self._request("GET", endpoint)

    def post(self, endpoint: str, data: Optional[Dict] = None) -> Dict:
        return self._request("POST", endpoint, data)

    def workflows(self) -> Dict:
        """List all workflows."""
        return self.get(f"repos/{self.owner}/{self.repo}/actions/workflows")

    def workflow_runs(self, workflow_id: str, branch: Optional[str] = None) -> Dict:
        """List runs for a workflow."""
        params = f"?branch={branch}" if branch else ""
        return self.get(
            f"repos/{self.owner}/{self.repo}/actions/workflows/{workflow_id}/runs{params}"
        )

    def get_run(self, run_id: int) -> Dict:
        """Get a specific run."""
        return self.get(f"repos/{self.owner}/{self.repo}/actions/runs/{run_id}")

    def get_run_logs(self, run_id: int) -> str:
        """Get logs for a run as a zip download URL."""
        return self.get(f"repos/{self.owner}/{self.repo}/actions/runs/{run_id}/logs")

    def trigger_workflow(
        self,
        workflow_id: str,
        ref: str = "fix/f001-panic-ndk-rootfix",
        inputs: Optional[Dict] = None,
    ) -> Dict:
        """Trigger a workflow dispatch."""
        data = {"ref": ref}
        if inputs:
            data["inputs"] = inputs
        return self.post(
            f"repos/{self.owner}/{self.repo}/actions/workflows/{workflow_id}/dispatches",
            data,
        )

    def get_jobs(self, run_id: int) -> Dict:
        """Get jobs for a run."""
        return self.get(f"repos/{self.owner}/{self.repo}/actions/runs/{run_id}/jobs")

    def get_job_logs(self, job_id: int) -> str:
        """Get logs for a job."""
        return self.get(f"repos/{self.owner}/{self.repo}/actions/jobs/{job_id}/logs")


class GitHubAPIError(Exception):
    """GitHub API error."""

    pass


class CI:
    """
    High-level CI interface for AI agents.

    Example:
        ci = CI()  # Uses GITHUB_TOKEN env var
        ci.trigger("aura-android-validate.yml")
        run = ci.wait_for_completion(timeout=300)
        if run.conclusion == "failure":
            logs = ci.get_logs(run.id)
            print(ci.analyze_failure(logs))
    """

    def __init__(
        self,
        token: Optional[str] = None,
        owner: str = "AdityaPagare619",
        repo: str = "aura",
    ):
        self.token = token or os.environ.get("GITHUB_TOKEN")
        if not self.token:
            raise ValueError("GITHUB_TOKEN not set. Run: export GITHUB_TOKEN=ghp_xxx")
        self.api = GitHubAPI(self.token, owner, repo)
        self.owner = owner
        self.repo = repo

    def list_workflows(self) -> List[str]:
        """List all available workflow files."""
        data = self.api.workflows()
        return [w["path"] for w in data.get("workflows", [])]

    def get_workflow_id(self, workflow_name: str) -> str:
        """Get workflow ID from name or path."""
        data = self.api.workflows()
        for w in data.get("workflows", []):
            if workflow_name in (w["name"], w["path"]):
                return str(w["id"])
        raise ValueError(f"Workflow '{workflow_name}' not found")

    def trigger(
        self,
        workflow: str,
        branch: str = "fix/f001-panic-ndk-rootfix",
        wait: bool = True,
        timeout: int = 600,
    ) -> Optional[WorkflowRun]:
        """
        Trigger a workflow and optionally wait for completion.

        Args:
            workflow: Workflow file name or path
            branch: Branch to run on
            wait: Wait for completion
            timeout: Max seconds to wait

        Returns:
            WorkflowRun object, or None if not waiting
        """
        workflow_id = self.get_workflow_id(workflow)

        print(f"Triggering {workflow} on {branch}...")
        self.api.trigger_workflow(workflow_id, ref=branch)
        print(f"Workflow triggered successfully")

        if not wait:
            return None

        # Poll for completion
        return self.wait_for_completion(workflow, branch, timeout)

    def wait_for_completion(
        self,
        workflow: str,
        branch: str = "fix/f001-panic-ndk-rootfix",
        timeout: int = 600,
        poll_interval: int = 15,
    ) -> Optional[WorkflowRun]:
        """Wait for workflow to complete."""
        workflow_id = self.get_workflow_id(workflow)
        start = time.time()

        while time.time() - start < timeout:
            data = self.api.workflow_runs(workflow_id, branch=branch)
            runs = data.get("workflow_runs", [])

            if runs:
                latest = runs[0]
                status = latest["status"]
                conclusion = latest.get("conclusion")

                print(f"  Status: {status} | Conclusion: {conclusion or 'pending'}")

                if status == "completed":
                    return WorkflowRun(
                        id=latest["id"],
                        name=latest["name"],
                        status=status,
                        conclusion=conclusion,
                        head_sha=latest["head_sha"],
                        branch=latest["head_branch"],
                        created_at=latest["created_at"],
                        updated_at=latest["updated_at"],
                        html_url=latest["html_url"],
                        logs_url=latest["logs_url"],
                    )

            time.sleep(poll_interval)

        print(f"Timeout after {timeout}s")
        return None

    def get_logs(self, run_id: int) -> str:
        """Get combined logs for a run."""
        jobs_data = self.api.get_jobs(run_id)

        combined_logs = []
        for job in jobs_data.get("jobs", []):
            job_id = job["id"]
            job_name = job["name"]
            job_status = job["status"]
            job_conclusion = job.get("conclusion")

            combined_logs.append(f"\n{'=' * 60}")
            combined_logs.append(
                f"JOB: {job_name} | {job_status} | {job_conclusion or 'pending'}"
            )
            combined_logs.append(f"{'=' * 60}")

            # Get step details
            for step in job.get("steps", []):
                step_name = step["name"]
                step_status = step["status"]
                step_conclusion = step.get("conclusion")
                step_number = step["number"]

                combined_logs.append(f"\n  STEP {step_number}: {step_name}")
                combined_logs.append(
                    f"    Status: {step_status} | {step_conclusion or 'pending'}"
                )

                # Try to get logs for this step
                if step_status == "completed" and step_conclusion == "failure":
                    # For failed steps, we need to get step-level logs
                    combined_logs.append(
                        f"    FAILED — check GitHub Actions UI for details"
                    )

        return "\n".join(combined_logs)

    def get_artifacts(self, run_id: int) -> List[Dict]:
        """Get artifacts from a run."""
        data = self.api.get(
            f"repos/{self.owner}/{self.repo}/actions/runs/{run_id}/artifacts"
        )
        return data.get("artifacts", [])

    def analyze_failure(self, logs: str) -> Dict[str, Any]:
        """
        Analyze CI failure logs and classify the failure type.
        Uses the taxonomy system to identify root cause.
        """
        from .taxonomy import TaxonomyClassifier

        classifier = TaxonomyClassifier()
        classification = classifier.classify(logs)

        # Extract additional context
        analysis = {
            "classification": classification,
            "recommendations": [],
        }

        # Add specific recommendations based on failure type
        failure_id = classification.get("id", "F999")

        if failure_id == "F001":
            analysis["recommendations"] = [
                "Check Cargo.toml for lto=true + panic=abort",
                "Verify NDK r26b container is being used",
                "Run: cargo test --test test_ndk_lto",
            ]
        elif failure_id == "F002":
            analysis["recommendations"] = [
                "Check memory usage in container with --memory=512m",
                "Review heap allocations in Rust code",
                "Consider using jemalloc for bionic",
            ]
        elif failure_id == "F003":
            analysis["recommendations"] = [
                "Verify Termux HOME is set: echo $HOME",
                "Check Termux storage permissions: termux-setup-storage",
                "Verify binary has execute permission",
            ]
        elif failure_id == "F999":
            analysis["recommendations"] = [
                "Unknown failure — manually inspect logs",
                "Check GitHub Actions UI for full error output",
                "Consider adding new entry to taxonomy",
            ]

        return analysis

    def get_status_badge(self, workflow: str) -> str:
        """Get GitHub Actions status badge URL."""
        workflow_id = self.get_workflow_id(workflow)
        return f"https://github.com/{self.owner}/{self.repo}/actions/workflows/{workflow_id}/badge.svg"


def main():
    """CLI interface."""
    parser = argparse.ArgumentParser(description="AURA CI Toolkit")
    sub = parser.add_subparsers(dest="command")

    # Token management
    token_parser = sub.add_parser("token", help="Set/get GitHub token")
    token_parser.add_argument("--set", type=str, help="Set token")
    token_parser.add_argument(
        "--show", action="store_true", help="Show current token (masked)"
    )

    # Trigger
    trigger_parser = sub.add_parser("trigger", help="Trigger a workflow")
    trigger_parser.add_argument("--workflow", required=True, help="Workflow name")
    trigger_parser.add_argument("--branch", default="fix/f001-panic-ndk-rootfix")
    trigger_parser.add_argument("--no-wait", action="store_true")

    # Status
    status_parser = sub.add_parser("status", help="Check workflow status")
    status_parser.add_argument("--workflow", required=True)
    status_parser.add_argument("--branch", default="fix/f001-panic-ndk-rootfix")

    # Logs
    logs_parser = sub.add_parser("logs", help="Get run logs")
    logs_parser.add_argument("--run-id", type=int, required=True)

    # Analyze
    analyze_parser = sub.add_parser("analyze", help="Analyze failure")
    analyze_parser.add_argument("--run-id", type=int, required=True)

    args = parser.parse_args()

    if args.command == "token":
        if args.set:
            print(f"Token set (masked): {args.set[:4]}***{args.set[-4:]}")
        elif args.show:
            token = os.environ.get("GITHUB_TOKEN", "")
            print(f"Token: {token[:4]}***{token[-4:]}" if token else "No token set")
        return

    # Get token for other commands
    token = os.environ.get("GITHUB_TOKEN")
    if not token:
        print("Error: GITHUB_TOKEN not set. Run: export GITHUB_TOKEN=ghp_xxx")
        sys.exit(1)

    ci = CI(token)

    if args.command == "trigger":
        ci.trigger(args.workflow, args.branch, wait=not args.no_wait)

    elif args.command == "status":
        run = ci.wait_for_completion(args.workflow, args.branch, timeout=30)
        if run:
            print(f"\nLatest run: {run.name}")
            print(f"  Status: {run.status}")
            print(f"  Conclusion: {run.conclusion}")
            print(f"  URL: {run.html_url}")

    elif args.command == "logs":
        logs = ci.get_logs(args.run_id)
        print(logs)

    elif args.command == "analyze":
        logs = ci.get_logs(args.run_id)
        analysis = ci.analyze_failure(logs)
        print(json.dumps(analysis, indent=2))

    else:
        parser.print_help()


if __name__ == "__main__":
    main()
