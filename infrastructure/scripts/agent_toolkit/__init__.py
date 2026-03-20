"""
AURA Agent Toolkit

This package provides tools for AI agents to interact with:
- GitHub Actions CI/CD
- BrowserStack device testing
- Failure taxonomy classification

Usage:
    from agent_toolkit import CI, BrowserStack, Taxonomy

    ci = CI(token="ghp_xxx")
    ci.trigger_build(workflow="aura-android-validate.yml")

    bs = BrowserStack(username="user", key="key")
    bs.run_live_session("Samsung Galaxy S24", "Android 14")
"""

from .ci import CI
from .browserstack import BrowserStack
from .taxonomy import TaxonomyClassifier

__all__ = ["CI", "BrowserStack", "TaxonomyClassifier"]
