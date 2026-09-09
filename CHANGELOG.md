# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions are not tagged yet; the sections below are curated history relocated
from source comments by the loom placement lint, each with its source provenance.

## Relocated from src/training.rs:702-712 (loom lint F-03, ratified 2026-09-09)

`direction: "decrease"` (contact_signal): only a DROP counts as
contact. This is what makes a health-valued signal (MK2's struct
health) immune to both INCREASE hazards by one sign check — the
round-intro ramp (+2/frame under the banner-gate leak) and the
training refill writing health back to max. Increases also don't
stamp the quiet-window bookkeeping, so a refill can't hold the
cooldown open. ACCEPTED LOSS: the hit that drives health below the
refill threshold is overwritten back to max by refill before the next
poll, so ~one real trigger per refill cycle is lost — the inverse of
the previously documented "one spurious punish per refill", and
harmless (the dummy blocks that one instead of punishing).
