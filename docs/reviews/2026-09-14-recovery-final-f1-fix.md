# Final Codex F1 correction

The retained picker row can now focus its already open recovered document after a successful v2 handoff retires the original source. `recovery_flow::import::drive` checks the exact selection token against live buffers before allocating or dispatching preparation. Only clean documents or documents with an acknowledged checkpoint at the current document version qualify. The predicate is shared with the post-prepare retry guard.

This focus path performs no filesystem calls, creates no request, emits no Open event, and acquires no deletion authority. Failed or unprotected recovery retries still use source preparation and revalidation. Focusing a protected document preserves an existing ordinary checkpoint request and its generation/routing unchanged.

Evidence under `evidence/2026-09-14-recovery-implementation/`:

- `task-final-f1-red.log`: new retired-v2 repeat regression fails before the production change because the active document remains the disk buffer.
- `task-final-f1-green.log`: all 20 handoff tests pass, including existing failed/unprotected retry and fault matrices.

New regression imports a v2 source through successful retirement, switches back to another document, reselects the original candidate, and proves direct focus with no extra request, checkpoint generation, document, or Open event, preserved successor, and source still retired. It additionally proves busy checkpoint routing is untouched and a subsequently clean document can focus without a retained Ack.

No snapshot evidence was rewritten. Final independent re-review and coordinator checks remain required.
