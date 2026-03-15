---
name: Bug Report
about: Report a reproducible bug in AURA v4
title: '[BUG] '
labels: ['bug', 'needs-triage']
assignees: AdityaPagare619
---

## Bug Description

<!-- A clear, concise description of what the bug is. -->

## AURA Version

<!-- Run: aura-daemon --version -->
Version:
Channel (stable/nightly):
Install method (install.sh / manual build):

## Device Information

```
Android version:
Device model:
Architecture: (should be arm64-v8a)
Available RAM:
Available storage:
Termux version:
```

## Steps to Reproduce

1.
2.
3.

## Expected Behaviour

<!-- What should have happened? -->

## Actual Behaviour

<!-- What actually happened? -->

## Install Log

<!-- Share your install log if the bug occurred during installation.
     Location: printed during install as "Install log: /tmp/aura-install-YYYYMMDD-HHMMSS.log"
     Paste the LAST 50 lines here, or attach the full file. -->

<details>
<summary>Install log (last 50 lines)</summary>

```
paste here
```

</details>

## Runtime Logs

<!-- Share daemon logs if the bug occurs at runtime.
     Location: ~/.local/share/aura/logs/current
     Run: tail -n 100 ~/.local/share/aura/logs/current -->

<details>
<summary>Daemon logs (last 100 lines)</summary>

```
paste here
```

</details>

## Config File

<!-- Share your config (remove any sensitive values first):
     Location: ~/.config/aura/config.toml -->

<details>
<summary>config.toml (redacted)</summary>

```toml
paste here
```

</details>

## Additional Context

<!-- Screenshots, related issues, workarounds you've tried, etc. -->

---

**Privacy reminder:** Do not include your Telegram Bot Token, PIN, or any personal data in this issue. The config file section above should be redacted before sharing.
