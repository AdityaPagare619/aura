# Postmortem Template

**Document**: `docs/incidents/POSTMORTEM-TEMPLATE.md`  
**Version**: 4.0.0-stable  
**Date**: 2026-03-22  
**Status**: ACTIVE  
**Owner**: QA Validation Charter

---

## Postmortem: [Incident Title]

**Date**: YYYY-MM-DD  
**Duration**: X hours Y minutes  
**Severity**: [P1/P2/P3/P4]  
**Status**: [Postmortem Complete]  

---

## 1. Summary

**What happened?**

[One paragraph describing the incident in plain language]

**Impact:**
- Users affected: [number or "all users"]
- Duration of impact: [time period]
- Service degradation: [full outage / partial / degraded]

**Root cause (initial):**
[One sentence stating the root cause]

---

## 2. Timeline

| Time (UTC) | Event |
|------------|-------|
| YYYY-MM-DD HH:MM | [Event description] |
| YYYY-MM-DD HH:MM | [Event description] |
| YYYY-MM-DD HH:MM | [Detection] |
| YYYY-MM-DD HH:MM | [Mitigation applied] |
| YYYY-MM-DD HH:MM | [Service restored] |

---

## 3. Detection

**How was the incident detected?**
- [ ] User report
- [ ] Monitoring alert
- [ ] Automated test failure
- [ ] Team member discovery

**Detection time**: [time from incident to detection]

---

## 4. Impact Assessment

| Metric | Value |
|--------|-------|
| Users impacted | [number] |
| Duration | [duration] |
| Revenue/data loss | [if applicable] |
| CI/CD blocked | [yes/no, duration] |

---

## 5. Root Cause Analysis

### 5.1 Failure Classification

**Failure Code**: [F001-F015 from FAILURE_TAXONOMY.md]

**Layer**: [Build / CI / Runtime / Device / Process]

**Root Cause (detailed):**

[Detailed explanation of what caused the incident. Address:]
- What was the immediate cause?
- What was the underlying cause?
- Why did the underlying cause exist?

### 5.2 Contributing Factors

1. [Factor 1]
2. [Factor 2]
3. [Factor 3]

### 5.3 Why Detection Failed

[If applicable - why wasn't the issue caught before production?]

---

## 6. Resolution

**Mitigation applied:**
[What was done to stop the bleeding]

**Fix implemented:**
[What was done to prevent recurrence]

---

## 7. Lessons Learned

### What went well?
- [Positive observation 1]
- [Positive observation 2]

### What went poorly?
- [Negative observation 1]
- [Negative observation 2]

### Where were we lucky?
- [Fortunate circumstance that limited impact]

---

## 8. Action Items

| Item | Owner | Due Date | Status |
|------|-------|----------|--------|
| [Action description] | @user | YYYY-MM-DD | Open |
| [Action description] | @user | YYYY-MM-DD | Open |

---

## 9. Related Failures

**Connected F-codes:**
- [F001] - [description of connection]
- [F002] - [description of connection]

**Previously documented similar incidents:**
- [Link to previous postmortem]

---

## 10. Sign-off

| Role | Name | Date | Signature |
|------|------|------|-----------|
| Incident Lead | | | |
| Engineering | | | |
| QA Validation | | | |
| Owner/Approver | | | |

---

## Template Version History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-03-22 | Initial template |

---

**END OF TEMPLATE**
