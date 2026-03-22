# v4.0.0-stable Release Checklist

## Pre-Release
- [ ] All CI stages pass
- [ ] Compilation verified (all features)
- [ ] Tests pass (263+)
- [ ] Docs complete (7/7)

## Device Testing
- [ ] DT-001: Binary executes (exit 0)
- [ ] DT-002: Boot stages log (5/5)
- [ ] DT-003: curl_backend works

## Release
- [ ] Tag created (v4.0.0-stable)
- [ ] GitHub release drafted
- [ ] Rollback procedure documented
- [ ] Announcement sent

## Post-Release
- [ ] Monitor for 24 hours
- [ ] No new failures
- [ ] Update CHANGELOG.md
