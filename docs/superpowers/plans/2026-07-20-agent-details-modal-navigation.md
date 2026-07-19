# Agent Details Modal Navigation Implementation Plan

**Goal:** Cycle live Agent Planets while keeping Agent Details open and updated.

### Task 1: Route modal navigation

- Add failing reducer/CLI/UI tests for cyclic next/previous selection and modal content refresh while open.
- Permit `SelectNextAgent`/`SelectPreviousAgent` with details open; keep `SelectAgent` mouse path blocked.
- Route Tab/Down/j and Shift+Tab/Up/k through modal navigation; retain Enter/Esc/a and all other modal-local consumption.
- Run fmt/test/check/clippy and commit code.

### Task 2: Document and validate

- Add the behavior to current Agent Planets docs.
- Run release build and commit docs.
