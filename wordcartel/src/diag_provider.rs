//! The `DiagnosticsProvider` seam (Effort A): a thin, mockable trait behind which a diagnostics
//! backend runs. `NullProvider` is the hermetic default; `HarperLs` (harper_ls.rs) is the real one.
//! No merge/multi-provider machinery — harper is the only provider; the seam is Open-Closed
//! insurance for provider #2.
