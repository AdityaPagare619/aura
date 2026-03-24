# BrowserStack MCP — Complete Tool Reference

## Session Management
1. browserstack_runBrowserLiveSession — Desktop browser testing (Chrome/Firefox/Safari)
2. browserstack_runAppLiveSession — Mobile device interactive sessions (REQUIRES APK)
3. browserstack_runAppTestsOnBrowserStack — App Automate with Espresso/XCUITest
4. browserstack_setupBrowserStackAppAutomateTests — Appium SDK setup
5. browserstack_setupBrowserStackAutomateTests — Web automation setup

## App/Build Management  
6. browserstack_runAppTestsOnBrowserStack — Execute compiled tests on devices
7. browserstack_getBuildId — Get build ID by project + build name
8. browserstack_listTestIds — List test IDs by build + status

## Device Interaction
9. browserstack_fetchAutomationScreenshots — Get screenshots from sessions
10. browserstack_takeAppScreenshot — Take screenshot on device (needs APK)
11. browserstack_runAppLiveSession — Interactive session control
12. browserstack_startAccessibilityScan — Accessibility scan on URL

## Debugging
13. browserstack_getFailureLogs — Fetch logs (network/device/appium/crash)
14. browserstack_fetchRCA — Root Cause Analysis for test failures
15. browserstack_fetchSelfHealedSelectors — AI-healed selectors for flaky tests

## Visual Testing
16. browserstack_percyVisualTestIntegrationAgent — Percy visual diff
17. browserstack_expandPercyVisualTesting — Expand Percy coverage
18. browserstack_addPercySnapshotCommands — Add Percy snapshot commands
19. browserstack_runPercyScan — Run Percy visual build/scan
20. browserstack_fetchPercyChanges — Get visual changes between builds
21. browserstack_managePercyBuildApproval — Approve/reject Percy builds

## Test Management
22. browserstack_createProjectOrFolder — Create test projects/folders
23. browserstack_createTestCase — Create test cases
24. browserstack_updateTestCase — Update test cases
25. browserstack_listTestCases — List test cases by project
26. browserstack_createTestRun — Create test runs
27. browserstack_updateTestRun — Update test runs
28. browserstack_listTestRuns — List test runs
29. browserstack_addTestResult — Add test results to test run
30. browserstack_uploadProductRequirementFile — Upload PDR docs
31. browserstack_createTestCasesFromFile — Generate test cases from docs

## Low Code Automation
32. browserstack_createLCASteps — Generate LCA steps for test case

## Authentication
33. browserstack_createAccessibilityAuthConfig — Create auth config for scans
34. browserstack_getAccessibilityAuthConfig — Get auth config

## Analysis
35. browserstack_fetchBuildInsights — Build quality insights
36. browserstack_accessibilityExpert — A11y expert Q&A

---

## AURA v4 — Tool Selection Matrix

| What AURA Needs | Best MCP Tool | Alternative |
|---|---|---|
| Start Android session | runAppLiveSession | REST API |
| Install APK | runAppLiveSession (auto) | Upload via API |
| Run shell commands | NONE (App Live interactive) | Wrapper APK |
| Take screenshot | takeAppScreenshot | fetchAutomationScreenshots |
| Get device logs | getFailureLogs | BrowserStack dashboard |
| Root cause analysis | fetchRCA | Manual |
| Create test project | createProjectOrFolder | Done ✅ |
| Record test results | addTestResult | Done ✅ |

---

## APPROACH FOR AURA CLI BINARY:

AURA is NOT an APK — it's a CLI binary running in Termux. 
The ONLY way to test it via MCP is:
1. Build wrapper APK that executes the binary
2. Upload to BrowserStack  
3. Run via runAppTestsOnBrowserStack

Alternative (Partnership):
- You interact with App Live session (browser URL)
- I provide commands for you to type
- We capture screenshots to verify

