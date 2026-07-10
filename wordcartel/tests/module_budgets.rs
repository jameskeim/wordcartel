//! Module-size tripwire — the anti-regrowth GATE for the shell's dispatch hubs.
//!
//! Background (CLAUDE.md → "Module structure — dispatchers delegate, they don't
//! implement"): `app.rs`/`render.rs` grew into god-objects because they were *dispatch
//! attractors* — a central `match`/loop every feature had to edit, so they grew
//! monotonically. H1 (merge 304e263) seamed them back down (timers.rs table, reduce
//! Handled-skeleton, the leaves). These budgets bound the PRODUCTION size of the hubs so
//! the regrowth can't silently recur.
//!
//! A breach means: **extract a seam** (delegate the new behavior to a domain module or a
//! table row) — NOT reflexively raise the number. Bumping a budget is legitimate only
//! when a genuine split landed and the file is honestly near the cap; do it deliberately,
//! with a one-line rationale on the constant.

use std::path::Path;

/// Production line count = lines before the file's co-located `mod tests` block. Tests
/// inflate line count without inflating the responsibility we're bounding, so they don't
/// count toward a hub's production budget. (A handful of interspersed `#[cfg(test)]` items
/// — test-only imports, `step()` — are small and tolerated as noise.)
fn production_lines(src: &str) -> usize {
    let lines: Vec<&str> = src.lines().collect();
    match lines.iter().rposition(|l| l.trim_start().starts_with("mod tests")) {
        Some(i) => i, // lines [0, i) are production
        None => lines.len(),
    }
}

fn assert_hub_budget(rel: &str, budget: usize) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("module_budgets: cannot read {}: {e}", path.display()));
    let prod = production_lines(&src);
    assert!(
        prod <= budget,
        "{rel}: {prod} production lines exceeds its {budget}-line hub budget.\n\
         This is an anti-regrowth tripwire (CLAUDE.md → \"Module structure\"). Do NOT just\n\
         raise the budget — extract a seam: delegate the new behavior into a domain module\n\
         or a data-table row so the dispatcher stays thin. Only bump the budget if a real\n\
         split landed and the file is legitimately near the cap.",
    );
}

// Budgets set with headroom over the post-H1 production sizes (app 778 / render 756 /
// timers 197 as of merge 304e263) — tight enough to trip on real regrowth, loose enough
// that ordinary within-responsibility growth doesn't false-alarm.

#[test]
fn app_rs_stays_a_thin_dispatch_hub() {
    // app.rs — the reduce/run dispatch hub. Effort P wires plugins in HERE, so this is the
    // budget most at risk: plugin arms/hooks must register into a seam, not grow reduce/run.
    assert_hub_budget("src/app.rs", 1000);
}

#[test]
fn render_rs_stays_bounded() {
    // render.rs — the paint hub. The 361-line render() body split is the tracked H1
    // follow-up (engineering-health.md); it restructures within this file, budget holds.
    assert_hub_budget("src/render.rs", 900);
}

#[test]
fn timers_rs_grows_by_rows_not_bulk() {
    // timers.rs — the timed-subsystem table (the anti-regrowth seam itself). New subsystems
    // add a SUBSYSTEMS row + a small deadline fn, never bulk; this keeps the seam a seam.
    assert_hub_budget("src/timers.rs", 400);
}
