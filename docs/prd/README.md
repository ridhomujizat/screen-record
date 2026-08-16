# Product Requirement Documents

This directory holds PRDs for TesAPI features. Each PRD captures the *what* and *why* of a
product initiative — problem, goals, acceptance criteria, UX flows, data model, and the
implementation map — before the [ADRs](../adr/) and [diagrams](../diagram/) are written.

A PRD may change over the life of a feature; the ADRs it spawns are the immutable record of
*why* the spec is shaped the way it is (see [../adr/README.md](../adr/README.md)).

## Index

| Story | Title | Epic | Status |
|-------|-------|------|--------|
| PD-0001 | Screen Recorder (Tauri + React) | screen-record | Active |


## How to add a PRD

1. Create `PD-XXXX-kebab-case-title.md`.
2. Cover: context/problem, goals & non-goals, user story, acceptance criteria, UX flows,
   data model, API surface, subtask map, dependencies/open questions, verification.
3. Keep domain and UI labels verbatim from the story; prose in English.
4. Add a row to the index above.
5. When a decision in the PRD hardens, capture it as an ADR and cross-link both ways.
