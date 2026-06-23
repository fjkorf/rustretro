---
# Shared parent frontmatter for the RustRetro tutorial pages.
# This file has NO `page:` section and no stateful widgets — it is the
# `parent:` referenced by `define_markdown_app!` once litui is integrated
# (see ROADMAP → "litui integration"). Child pages inherit these styles.
styles:
  # Callout for the "Honest limit" / honest-limits notes the tutorials use.
  note:
    italic: true
    color: "#B8860B"
  # Numbered walkthrough steps.
  step:
    bold: true
    color: "#4A90D9"
---

RustRetro tutorials — task-oriented walkthroughs for taking a ROM apart while it plays.
