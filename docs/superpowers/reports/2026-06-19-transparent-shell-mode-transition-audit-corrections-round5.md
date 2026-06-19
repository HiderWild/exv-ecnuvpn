# Transparent Shell Mode Transition Audit Corrections - Round 5

Date: 2026-06-19

## Review Source

- Final documentation-focused Codex Spark audit after commit `16224a9`.
- GPT-5.4 adjudication of the remaining documentation mismatch.

## Confirmed Required Correction

### R5-1 Plan Task 6 template missed macOS mask-settle check

Status: Confirmed documentation drift.

The active manual verifier and host policy test require `macOS: app icon mask remains visible through transition settle.`, and the requirement was already accepted in round 2. The committed implementation plan's Task 6 checklist omitted that line.

Correction:

- Add the macOS mask-settle checklist item to the Task 6 manual acceptance template in the plan.

## Accepted Residuals

- macOS visual verification remains a release-signoff residual because this execution environment is Windows.
- Windows foreground focus remains subject to normal OS activation policy.

## Verification Plan

- Confirm the plan Task 6 checklist includes the macOS mask-settle line.
- Run `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts`.
- Run `git diff --check`.
