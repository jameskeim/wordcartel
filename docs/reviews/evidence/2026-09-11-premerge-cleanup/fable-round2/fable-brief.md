# Independent Fable follow-up — final cleanup delta

Review the current R8/R12 implementation on fix/quit-save-durability. The snapshot
is already applied in this isolated checkout; do not apply the diff again.

A full Fable review of the preceding 25-file snapshot returned GO with no Critical
or Important findings. Read previous-fable-review.md in this evidence directory,
then followup.diff (the changes since that review) and the actual current code.
The current 26 source-file hashes are in source-sha256.json. The full branch diff
is also available via git. Prior approval is context, not proof about the new changes.

Independently assess the delta and its interactions with the full implementation:

- M1: closing a quit-owned picker through the overlay registry now delegates to
  file_browser::close_overlay/cancel_destination. Successful commits remove the
  picker before opening overwrite confirmation; that transition must stay valid.
  Manual/non-owned picker closure retains its previous behavior.
- M2: remaining failure/panic/timeout drain cancellations use quit::cancel.
- M3: save_finished reports whether a failed plain-Quit wait was cancelled; the
  completion/panic status explicitly names the cancelled quit.
- M6/M7: one stale legacy comment and two guarded unwraps were clarified.
- Two maintained regressions cover overlay replacement and changed-destination
  cancellation wording; failure-wait coverage now also checks that wording.

Pre-existing landed-but-unmerged fingerprint prompts and quit-summary reentry
(prior M4/M5) are explicitly deferred, not claimed fixed. The timed benchmark
intentionally returns timings; quit state remains observable on Editor. The prior
unchecked-checklist observation concerned an old copy; the live checklist is updated.
Report any new functional issue you find, regardless of this scope description.

The current source passed 32 durability tests, 2 actual-plugin exit tests, and the
full workspace suite (2,498 passed, 6 ignored). Other gates are being refreshed on
this frozen source. The independent Codex delta review is codex-review-round3.md.
Run focused checks/probes where useful; there is no need to repeat unrelated earlier
investigation unless the delta warrants it. Read source before reaching a verdict.

The checkout, repar dependency, build target, and state/config/cache are isolated.
Do not access or modify the original checkout. Temporary tests may be added HERE;
preserve useful probes under review-probes/ and restore production source. Do not
implement fixes, run rustfmt, commit, push, merge, delegate, inspect credentials,
or contact other services. Python may clean up your own temporary files. Clippy
and the smoke runner are permitted if needed.

Write FABLE_REVIEW.md in the checkout: independent design-compliance and code-quality
verdicts, GO/NO-GO, Critical/Important/Minor findings with triggers and evidence,
checks actually run, and limitations. Distinguish retained observations from new
findings. If no new issues, say so. Return a concise final summary.
