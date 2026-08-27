---
name: wrap-session
description: End-of-session ritual — consolidate evidence docs, sweep the tree, write memory, and ship or queue the PR
---

# wrap-session — leave it better than a snapshot

## 1. Tree sweep
- `git status`: no tracked churn staged (busmap pretty-print rewrites, `__pycache__`
  .pyc files — never commit those). Untracked keepers get classified: personal config
  → gitignore; named arenas / fixtures → commit; scratch → job tmp.
- Full test suites green (`cargo test --profile release-dev`; `cd shadow/train &&
  .venv/bin/python3 -m pytest -q`) and the release-dev BINARY rebuilt if the user will
  play (`cargo build --profile release-dev` — tests alone don't rebuild it).
- Fossil grep on anything extracted this session: search for the old constants'
  PATTERN, not just green tests.

## 2. Evidence & docs
- Any `library/**/*.md` that accreted "session gotchas" strata gets a consolidation
  pass: current-truth table up top, topical sections, disproven/traps preserved,
  superseded values kept as explicit history. Never silently delete a claim.
- CLAUDE.md: update only what drifted (commands, flags, layout). It is a map — deep
  workflow belongs in these skills.

## 3. Memory
Update the project-arc memory file with: what LANDED (with commit refs), what's
PENDING (ordered), open bugs parked, and any new session-craft lesson (those go to the
workflow-lessons file with Why/How-to-apply). Convert relative dates to absolute.

## 4. Ship
- Nobody commits but the orchestrator; commits are per-logical-unit with evidence in
  the message (test counts, smoke results).
- PR when the phase is QA'd — the house QA bar is the user physically exercising the
  feature. Merge order matters for stacked branches. GitHub without `gh`: token via
  `git credential fill` piped to curl (never print the token); merge = PUT
  /pulls/N/merge.
- If the PR waits on user QA, say exactly what to test and leave the queue in memory
  so the next session opens with one obvious first move.
