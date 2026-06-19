# Transparent Shell Mode Transition Audit Corrections - Round 4

Date: 2026-06-19

## Review Source

- Post-`ec15acb` Codex Spark audits split by renderer/UI, native window behavior, and test/documentation/packaging coverage.
- GPT-5.4 adjudication of Spark findings.

## Confirmed Required Corrections

### R4-1 Plan execution status was not auditable

Status: Confirmed workflow/documentation defect.

The implementation plan existed only as an untracked file and kept its task steps unchecked, so the execution status could not be audited from the plan artifact itself.

Correction:

- Commit the plan artifact.
- Mark the task steps complete after implementation and verification.
- Add an explicit execution-status section summarizing completed local verification, audit loop status, and residual platform signoff.

### R4-2 Plan acceptance template missed outside-mask transparency

Status: Confirmed documentation drift.

The active manual verifier and host policy tests include the resize-transition outside-mask transparency requirement, but the plan's Task 6 acceptance template did not.

Correction:

- Add Windows and macOS outside-mask transparency checklist items to the Task 6 template.
- Add the same requirement to the Windows visual acceptance expected outcomes.

## Accepted Residuals

- macOS visual verification was not run in this Windows environment. This is a release-signoff residual, not a local completion blocker.
- Win32 `SetForegroundWindow` can still be subject to normal OS foreground-activation policy.
- Native visual/runtime invariants are covered by static checks and manual acceptance artifacts, not by a fully automated native-window visual harness in this tree.
- Current working-tree type drift in `webui/src/types/ecnu-vpn.d.ts` is unrelated to transparent-shell window APIs.

## Verification Plan

- Confirm the plan file contains completed task status and the explicit residual macOS visual signoff note.
- Confirm the plan Task 6 template includes outside-mask transparency checks.
- Run `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts`.
- Run `git diff --check`.
