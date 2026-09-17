# Task 6 picker and scan controller implementation

Implemented the picker/controller portion assigned by the coordinator. Parent owns startup/open-path wiring, legacy prompt removal outside Editor/overlays, cleaner integration and journey tests.

## Interfaces

- `recovery_flow::bootstrap(&mut Ctx)`: one All scan, dispatches before first receive; no installation or callbacks.
- `recovery_flow::opened(&mut Editor, BufferId, &Path)`: queue contextual scan. Queued duplicate intents coalesce; an already-running scan does not absorb a later intent because its snapshot may already be stale.
- `recovery_flow::review(&mut Ctx)`: shared registry command route. Manual All ignores session suppression; prompt/save/quit/import busy guards preserve current flow.
- `after_callbacks`: drives scans and safe-boundary offers. Scan completions use shared checked request IDs, separate scan epoch and exact-ID panic routing. Dismissed scan records are removed; late merges cannot resurrect UI.
- `protected_sources(&Editor) -> Vec<PathBuf>`: queued, preparing/ready, installed source paths. Import request carries path/token metadata; Buffer retains source path alongside existing token. No bodies or leases enter the picker.

## Lifecycle

Recovery is a complete overlay-table row, present in ALL and RENDER_ORDER. Shared geometry controls paint and mouse; viewports and previews are bounded. Up/Down/Space/Enter, zero selection, Esc/click-away/registry replacement, visible progress/empty/errors and changed-preview reselection are implemented. Date display uses UTC civil dates and explicitly labels legacy filesystem mtime.

Opening retains its picker through sequential prepare/handoff completion. Closing an Opening picker cancels its batch but retains installed documents. Closing a scan-only picker cannot cancel an unrelated installed handoff. Plugin replacement follows close_all and the same cancellation path. Terminal failures retain explicit row reports and clear selection; changed tokens update the preview without consent transfer.

Automatic scans filter active-owner busy rows, dismissed exact tokens, opened tokens and pending/live imports. Safe offers wait for other overlays and save/quit transitions; startup may replace splash without keyboard input. Contextual results whose buffer is inactive, closed or rekeyed never interrupt another document. Manual scan remains available for their preserved sources.

## Validation

- `task6-picker-red.log`: two actual behavioral assertion failures before implementation (dismissal and zero-selection feedback).
- `task6-picker-green.log`: 16 picker/controller tests pass, including sequential multi-import, separate document preservation, stale preview, prepare cancellation, installed handoff cancellation, scan-only cancellation isolation, manual reopen/focus, exact panic/epoch routing, contextual follow-up assessment, modal deferral, splash replacement and actual tiny/normal rendering plus mouse geometry.
- `task6-overlays-green.log`: 26 overlay-filter tests pass; shared per-overlay active/XOR/key/mouse/close sweeps now include Recovery.
- `task6-registry-green.log`: 82 registry-filter tests pass.
- `task6-picker-clippy.log`: crate lib/tests clippy passed after fixing assignment spacing. Subsequent scan coalescing/error-path changes are covered by the latest focused test run; parent final clippy remains required.

Logs are in `docs/reviews/evidence/2026-09-14-recovery-implementation/`.
No formatting command, commit, merge, push, task 7 timeout implementation or delegation was performed.
