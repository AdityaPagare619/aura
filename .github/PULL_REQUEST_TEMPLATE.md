## Pull Request

### Summary

<!-- One or two sentences: what does this PR do and why? -->

### Type of Change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that changes existing behaviour)
- [ ] Refactoring (no functional change, code quality improvement)
- [ ] Documentation update
- [ ] CI/DevOps change
- [ ] Dependency update

### Iron Laws Compliance

All PRs must comply with AURA's Iron Laws. Confirm each:

- [ ] **LLM = brain, Rust = body** — I have not added semantic reasoning, intent detection, or NLU logic to any Rust code
- [ ] **No cloud, ever** — I have not added any network calls, telemetry, external APIs, or cloud fallbacks
- [ ] **No theater AGI** — I have not added keyword matching, regex NLU, or hardcoded intent classification
- [ ] **No production logic changed to make tests pass** — all tests test real behaviour
- [ ] **Deny-by-default** — any new capabilities are explicitly allowlisted, not granted implicitly

### Changes Made

<!-- Bullet list of specific changes. Be concrete. -->

-
-

### Testing

- [ ] `cargo check --workspace --features stub` passes
- [ ] `cargo test --workspace --features stub` passes with no new failures
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] Relevant unit tests added or updated
- [ ] Integration tests pass (if applicable)

### Install Script (if touched)

- [ ] `bash install.sh --dry-run` produces no errors
- [ ] `shellcheck install.sh` passes with no warnings

### Related Issues

<!-- Closes #NNN or Relates to #NNN -->

### Notes for Reviewer

<!-- Anything specific you want the reviewer to look at? Trade-offs made? Alternatives considered? -->
