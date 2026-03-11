# CHECKPOINTS P3a + P3b vs EPSILON + ETA CROSS-VALIDATION

**Date**: 2026-03-10  
**Auditor**: Senior UX/Interface Auditor  
**Sources**: 2h-p3a-telegram-interface.md (318 lines), 2h-p3b-voice-reaction.md (431 lines)  
**Targets**: EPSILON-USER-EXPERIENCE-DESIGN.md (630 lines), ETA-VOICE-INTERFACE-DESIGN.md (755 lines)

---

## Findings Extracted

- From P3a (Telegram): **35 findings** (F1-F35)
- From P3b (Voice/Reaction): **20 findings** (V1-V20)
- Total: **55 findings**

---

## COVERED (44 findings)

| ID | Summary | Resolved In |
|----|---------|-------------|
| F1 | Personality expression 0/10 — OCEAN model unused in messages | Epsilon §2 Stage 3 Composition (L163-169), §3 (L182), Decision #1 (L615), #10 (L624) — PersonalityComposer replaces static templates with LLM + continuous OCEAN |
| F2 | No NLU / fuzzy matching — typo = parse failure | Epsilon §7 Two-Tier Input Processing (L480-503) — Tier 1 regex fast-path + Tier 2 LLM intent classification |
| F3 | No inline keyboards or rich Telegram UI | Epsilon §3 Rich UI (L188-198) — inline keyboards for dialogues, suggestions, morning briefing; P0 priority (L598) |
| F5 | No command suggestions on partial match | Epsilon §3 Error redesign (L271): "Did you mean `/[closest match]`?" |
| F6 | No context-aware command routing | Epsilon §4 Context-Awareness Stack (L324-330): time, activity, device, emotional state, conversation recency |
| F8 | No personality in dialogue prompts (clinical tone) | Epsilon §2 Stage 3 (L163-168): "every outbound message passes through PersonalityComposer" — dialogue prompts included |
| F9 | No typing indicators | Epsilon §3 Typing Indicators (L200-202): sendChatAction(typing) before >2s responses |
| F12 | No rich message formatting in prompts | Epsilon §3 Message Formatting (L194-198): bold, italic, emoji calibrated to Extraversion, code blocks restricted |
| F13 | Dashboard shows sysadmin data not user insights | Epsilon §5 complete redesign (L344-416), Decision #5 (L619) |
| F14 | No CPU/RAM/battery metrics | Epsilon §5 View 1 Daily Pulse (L352-365) — energy, focus, connections, goals |
| F15 | No LLM status | Epsilon §5 subsumes under system health — moved to /debug |
| F16 | No memory system stats | Epsilon §5 View 3 Character Sheet (L382-395) — what AURA has learned |
| F17 | No active goals or task progress | Epsilon §5 View 1 (L364): "Marathon training is on track — 3 runs this week" |
| F18 | No OCEAN personality snapshot | Epsilon §5 View 3 Character Sheet — includes Models layer (L392) |
| F19 | No relationship/trust level display | Epsilon §5 Growth Visualization (L399-404): "Your trust level: Companion (building toward Trusted Partner)" |
| F20 | No health tracking metrics | Epsilon §5 View 1 (L359): "you slept 7.5 hours" — health integrated |
| F21 | No proactive suggestions in dashboard | Epsilon §5 View 1 inline keyboards: [Show details] [What should I focus on?] |
| F22 | No proactive messaging — AURA never initiates | Epsilon §4 entire section (L275-341) + §2 Three-Stage Dispatcher. P0 priority (L597) |
| F23 | CLI wearing Telegram skin | Epsilon §3 (L175): "From CLI-in-Telegram-Skin to Conversational Companion" — directly named and addressed |
| F25 | Slash-command UX ceiling at 43 commands | Epsilon §7 "Natural language first, commands second" (L181), Decision #3 (L617) |
| F26 | No ProactiveEngine → Telegram bridge | Epsilon §2 entire dispatcher architecture, P0 priority (L597) |
| F27 | Dashboard information gap | Epsilon §5 redesign — user-facing insights replace sysadmin metrics |
| F28 | PersonalityFormatter needed for outgoing messages | Epsilon P0 PersonalityComposer (L596) — highest priority |
| F29 | User-facing dashboard needed | Epsilon §5 Daily Pulse, P1 priority (L600) |
| F30 | Morning briefing command needed | Epsilon §4 (L290-293) morning briefing, P1 (L599), conversation example (L207-213) |
| F31 | Command suggestion engine needed | Epsilon §3 error redesign (L271) |
| F32 | Conversation context (last 5 messages) needed | Epsilon §7 Multi-Turn (L505-513): 10-message context window (exceeds audit's 5-message suggestion) |
| F33 | NLU layer for intent classification | Epsilon §7 Tier 2 LLM (L488-493), P2 priority (L602) |
| F35 | Rich message templates with personality variants | Epsilon §3 Personality Expression (L238-262) — LLM-composed, not template-based |
| V1 | Voice maturity 4/10 — no audio flows | Eta §1 (L38-43) acknowledges, targets 7/10 (L65) with 5-phase roadmap |
| V2 | STT not running — FFI declared, TODOs at init | Eta §2 (L128-200) full STT design, Phase 2 roadmap (L722-727) |
| V3 | TTS not running — no speech synthesis | Eta §3 (L203-296) full TTS design, Phase 1 roadmap (L715-720) |
| V4 | Wake word not functional — mock only | Eta §2 Wake Word (L139-153), Phase 3 roadmap (L729-733) |
| V5 | biomarkers.rs sleeper weapon — never runs on real data | Eta §4 (L299-389) full activation design, Phase 4 (L735-740) |
| V6 | Critical TODO mod.rs:214 — mood hardcoded, not Amygdala | Eta §8 Gamma integration (L664): "Fixes the critical TODO at mod.rs:214." Phase 4 (L736) |
| V7 | Reaction detection text-only, no voice features | Eta Phase 4 (L737): "Feed biomarker output to reaction.rs." §5 Multimodal (L446-448) |
| V8 | Silero VAD not wired | Eta §2 VAD (L155-165), Phase 5 (L743) |
| V9 | Oboe audio I/O declared but not wired | Eta §3 Audio Output (L288-295), Phase 1 (L718) |
| V10 | RNNoise declared but not linked | Eta Phase 5 (L744) |
| V11 | personality_voice.rs unused — no TTS engine | Eta §6 (L468-535) full design, Phase 1 (L719) connects it to Piper |
| V16 | Wire TTS first as fastest path to F.R.I.D.A.Y. | Eta §3 (L205-210) "TTS Is The Highest-Priority Voice Feature" — adopted exactly |
| V17 | Feed biomarkers into reaction.rs for empathy | Eta Phase 4 (L737), §5 Multimodal (L446-448) |
| V19 | Progressive voice personality evolution | Eta §3 Relationship-Stage Voice Evolution (L274-285) — explicit trust-to-warmth mapping |
| V20 | Memory budget ~143MB feasible | Eta §7 (L540-561) confirms, provides detailed peak-vs-effective breakdown |

---

## GAPS (4 findings)

| ID | Summary | Missing — What Should Be Added |
|----|---------|-------------------------------|
| F7 | Only 2 of 4 declared dialogue flows implemented (AutomateSetup, PinChange) | MISSING — Epsilon redesigns dialogue UX but never mentions completing the 2 unimplemented flows. Epsilon should specify whether these flows are kept, redesigned, or dropped. |
| F10 | No branching dialogues (linear steps only) | MISSING — Epsilon §7 covers multi-turn conversation but doesn't address branching within FSM dialogue flows. Should specify: does the PersonalityComposer + NLU replace the FSM, or does the FSM get branching support? |
| F11 | No "undo" / go-back within dialogue flow | MISSING — Not mentioned anywhere in Epsilon. Dialogue UX should support step-back ("wait, I meant the other one") within multi-step flows. |
| V13 | SSML support planned but not implemented in TTS | MISSING — Eta designs TTS thoroughly but never mentions SSML. For personality expression via emphasis/pauses, SSML-like control would be valuable. Should Piper use SSML, or is the personality_voice.rs parameter approach sufficient? |

---

## PARTIAL (7 findings)

| ID | Summary | Covered | Missing |
|----|---------|---------|---------|
| F4 | No /morning, /briefing, /mood, /undo, /teach, /goals, /habits, /journal, /dream, /trust commands | Morning briefing covered (Epsilon §4, L290). Mood handled via emotional NLU (Epsilon §7, L500). | /undo, /teach, /goals, /habits, /journal, /dream, /trust not mentioned. These lifestyle commands that make AURA feel like a companion are not designed. NLU may absorb some, but explicit quick-access commands matter for power users. |
| F24 | Telegram platform dependency — no fallback UI | Android notification as CRITICAL fallback (Epsilon §4, L159). OutboundDispatcher is conceptually channel-agnostic (Epsilon §8, L537). | No explicit multi-platform abstraction design. No Signal, local CLI, or web UI fallback. No "what happens if Telegram goes down" contingency beyond Android notifications. |
| F34 | Multi-platform MessageChannel trait abstraction | Epsilon §8 describes channel selection logic (L152-159) with Telegram + Android notifications. | No MessageChannel trait or platform abstraction layer designed. Epsilon assumes Telegram as primary with no explicit abstraction point for future channels. |
| V12 | Call handler works via A11Y but no voice pipeline active | Eta §5 references modality_state_machine InCall state (L29, L419). State machine handles call interrupts. | No specific redesign of call_handler.rs integration with the new voice pipeline. How does an active voice conversation interact with an incoming phone call? The state machine handles transitions, but the actual audio routing during/after calls isn't specified. |
| V14 | No language detection | Eta §2 (L189-191): "Language detection runs on first 3 seconds of speech" mentioned briefly. | One sentence, no design. How does language detection work? What languages are supported? What happens when non-English is detected? Does it switch Whisper models? No implementation detail. |
| V15 | Voice fingerprint for security (speaker verification) | Eta §4 (L370-372): acknowledges the possibility, says not by default. | No design for opt-in voice-based speaker verification. The audit suggested using biomarkers for live similarity checks without stored biometrics — a novel approach that Eta acknowledges but doesn't design. |
| V18 | Connect voice biomarkers to TRUTH Protocol for wellbeing | Eta §4 Ethical Boundaries (L377-388) references TRUTH protocol. Wellbeing nudges described (L387). Biomarker trends feed Health domain (L691). | The audit's specific suggestion — "If user sounds exhausted at 2 AM: 'Maybe we should continue this tomorrow?'" — routes through Health domain generically, not through a dedicated TRUTH Protocol integration for voice-detected states. The directness of voice→TRUTH→intervention is diluted through domain routing. |

---

## AGI PHILOSOPHY VIOLATIONS

| Location | Violation | Should Be |
|----------|-----------|-----------|
| **Epsilon §2, L113** | Hardcoded rate limits per trust tier: "3 proactive/day at Stranger-Acquaintance, 5 at Companion, 8 at Trusted Partner-Kindred" — fixed numbers bound to tier labels | Starting values that **adapt** based on engagement feedback. Epsilon §2 L117 already has a feedback loop ("reduce frequency of ignored categories by 20%/week") — the per-tier caps should also be learnable. Specify initial values as defaults with adaptive drift based on user tolerance. |
| **Epsilon §6, L470** | Hardcoded thought bubble ratio: "No more than 1 thought bubble per 5 messages" — fixed ratio regardless of user engagement | Adaptive ratio that learns from user response. If user loves thought bubbles (high engagement), allow 1-per-3. If user toggles them off once, reduce to 1-per-10 before showing again. The ratio should be a starting point, not a ceiling. |
| **Eta §3, L278-282** | Hardcoded personality expression offsets per trust tier: "warmth: +0.1" at Acquaintance, "+0.2" at Trusted Partner — fixed increments per stage | Continuous interpolation using trust score (0.0-1.0) directly, as Gamma's blending system already supports. Eta L284 references `blend_profiles()` for smooth interpolation — the stage-based fixed offsets contradict this. Should be: `warmth_boost = trust_score * max_warmth_boost` with learned `max_warmth_boost`. |

**Note on exceptions applied**: Eta §4's confidence thresholds (>0.7, 0.4-0.7, <0.4) and temporal smoothing windows (5-second, 2-second persistence) are **not violations** — they fall under the "Audio processing parameters / signal processing constants" exception. Similarly, Eta §7's latency targets and memory budgets are engineering constraints, not behavioral hardcoding.

---

## PERSONALITY EXPRESSION ANALYSIS

### Does Epsilon solve "CLI wearing Telegram skin"?

**YES** — Comprehensively addressed.

Evidence:
- Epsilon §3 is literally titled "From CLI-in-Telegram-Skin to Conversational Companion" (L175)
- PersonalityComposer wraps ALL outgoing messages through LLM + continuous OCEAN (§2 Stage 3, L163-169)
- NLU makes natural language the primary input, slash commands become power-user shortcuts (§7, L181)
- Proactive messaging means AURA initiates conversations (§4, L275-341)
- Inline keyboards, typing indicators, rich formatting replace plain text (§3, L188-202)
- Conversation examples (L207-235) show the target experience — warm, personality-rich, contextual
- 10 Key Design Decisions (L611-624) directly address every dimension of the CLI problem

### Does Epsilon integrate with Gamma's personality system?

**YES** — Epsilon calls Gamma its "deepest integration" (L551).

Evidence:
- Continuous OCEAN interpolation from Gamma §3 feeds PersonalityComposer (L555)
- Relationship stage modulates tone — Stranger=formal, Kindred=warm shorthand (L165-166)
- Trust level checks gate all Stage 2 evaluation decisions (L557)
- AURA's 5 character traits explicitly mapped to message patterns (§3, L238-247)
- Thought bubble visibility governed by Gamma's trust rules (L558)
- Trust feedback flows back to Gamma from user responses (L559)
- Decision #10 (L624): "Composition uses LLM with continuous OCEAN parameters"

### Does Eta activate voice biomarkers.rs?

**YES** — Eta §4 is entirely dedicated to activating biomarkers.

Evidence:
- Eta §4 titled "Voice Biomarker Analysis (The Sleeper Weapon)" (L299) — directly adopts audit's framing
- Full feature extraction pipeline designed (L307-331)
- Biomarker-to-emotion mapping table with confidence levels (L336-343)
- Confidence thresholds and temporal smoothing specified (L345-356)
- Privacy-first architecture: extract features → discard raw audio (L362-363)
- Ethical boundaries for emotional data use (L377-388)
- Phase 4 roadmap explicitly wires biomarkers (L735-740)
- BiomarkerSignal published to EventBus for cross-system consumption (L647)

### Is voice analysis connected to emotional context?

**YES** — Multi-path connection designed.

Evidence:
- Eta §5 Multimodal (L446-448): "User types 'I'm fine' but sounds stressed → AURA weighs both signals"
- Eta §8 Beta integration (L654): "BiomarkerSignal { arousal, valence, stress, fatigue } enriches the cognitive context"
- Eta §8 Gamma integration (L664): Amygdala mood state overlays voice parameters — fixes mod.rs:214
- Eta §4 confidence-gated action: high confidence → adjust voice tone, medium → supplement text analysis, low → temporal smoothing only (L348-353)
- Eta §8 Zeta integration (L691): long-term biomarker trends feed Health domain for wellness insights
- Epsilon §8 Eta integration (L587): "Vocal biomarkers feed into emotional context for delivery decisions"

---

## SUMMARY

| Metric | Count | Percentage |
|--------|-------|------------|
| **Total findings** | **55** | 100% |
| **Covered** | **44** | 80% |
| **Partial** | **7** | 13% |
| **Gaps** | **4** | 7% |
| **AGI violations** | **3** | — |

### Assessment

The Epsilon and Eta redesign documents demonstrate **strong audit coverage** at 80% fully covered. The known critical issues — "CLI wearing Telegram skin" (personality 0/10), "operationally inert" voice pipeline, biomarkers.rs as sleeper weapon — are all directly and thoroughly addressed.

**What's excellent:**
- Epsilon directly names and solves every major Telegram finding (personality, proactive messaging, NLU, dashboard)
- Eta adopts the audit's "wire TTS first" strategy and designs the full biomarker activation path
- Both documents cite specific audit line numbers — they were clearly written as direct responses
- Cross-team integration tables (Epsilon §8, Eta §8) create accountability for integration points

**What needs attention:**
- 4 gaps are minor but worth tracking: incomplete dialogue flows (F7), no branching dialogues (F10), no undo in flows (F11), no SSML mention (V13)
- 7 partial items mostly reflect depth-of-design gaps rather than awareness gaps — the teams know about them but didn't fully specify
- 3 AGI violations are all fixable with "make these adaptive starting points rather than fixed constants"

**Verdict: B+ audit findings → A- redesign coverage.** The redesign documents close the critical gaps and establish a clear path from "working tool" to "conversational companion." Remaining gaps are implementation-level details, not architectural misses.

---

*Cross-validation completed 2026-03-10*
*Sources: P3a (318L), P3b (431L), Epsilon (630L), Eta (755L) — 2,134 total lines analyzed*
