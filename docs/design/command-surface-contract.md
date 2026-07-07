# Command-Surface Contract

**Status:** governing App contract (authoritative). Changes are deliberate — treat like an ADR.
**Scope:** how commands, the command palette, the menu, and keybinding hints relate. Every effort
that touches commands, user-settable options, the palette, the menu, or keybinding hints MUST
conform. Specs and plans state their conformance (see `CLAUDE.md` → Development process).

Wordcartel has three command surfaces — the **palette**, the **menu**, and **keybindings** — plus
a fourth actor coming with Effort P: **plugins** (in-process Lua). The command registry is the
single spine all four route through. This contract keeps them coherent.

---

## Why this exists

Two failures this contract prevents:
1. **Orphaned options.** The ZEN/FULL density preset (E1) added `status_line` and `scrollbar` modes
   as runtime fields + config keys with NO individual command — reachable only through the profile
   toggle or by hand-editing config. That is invisible to the palette and the menu, and (see below)
   uncontrollable by a plugin.
2. **Surface drift.** A profile that sets state through a path the individual command doesn't use
   can diverge from that command; a hint that doesn't track the active keymap misleads.

The deeper reason: **the command registry is the automation/plugin API.** A plugin changes editor
state by *dispatching commands*. So "every option is a command" is not a palette nicety — it is what
makes every option scriptable and plugin-controllable. Commands are the one verb layer through which
the palette (keyboard), the menu (mouse), keybindings (muscle memory), and plugins (automation) all
act.

---

## The laws (invariants — a violation is a bug; each has an enforcing test)

1. **The registry is the single source of truth.** Every command lives in the registry; nothing
   dispatches or mutates command-reachable state outside it.
2. **Every user-settable option is a command.** If a setting persists at runtime (a
   `SettingsSnapshot` field / a config key), a command changes it. *Test:* every persisted setting
   maps to a command / command-surface (the recurrence guard).
3. **The palette is exhaustive.** Every registered command that is not explicitly internal appears
   in the palette. *Test:* palette-completeness ("every non-hidden registry command appears";
   formalized from `palette.rs:138`).
4. **The menu is a curated subset.** menu ⊆ palette, always.
5. **Every mouse affordance has a keyboard path.** (Falls out of law 3.)
6. **One setter per option; profiles use it too.** State mutation for an option flows through a
   single setter function; a preset/profile changes the option by calling the *same* setter its
   command calls — never a bypass. A profile is "a batch of the setters a plugin could also call."
7. **Hints track the active keymap.** Both the palette and the menu show the chord from the *active*
   `KeyTrie`, re-resolved on a preset switch, and **prefer the user's explicit (patch-bound) binding
   over an inherited default**. *Tests:* hints re-resolve after a CUA↔WordStar switch; a custom bind
   surfaces in both surfaces.

---

## Shape rules (how a command is built)

8. **Multi-state option = set-value primitives + a stateful menu representative.** Provide explicit
   **set-per-state** commands (deterministic — automation needs "set to X," not "cycle and hope"; tag
   them `menu: None` → palette-only) **plus one stateful menu representative** — a **toggle** for a
   2-state option, a **cycle** for a 3+-state one — carried in the menu with state-in-label. The menu
   representative is a convenience: it need NOT expose every state directly (the palette does, via the
   set-per-state commands). Precedents: `toggle_chrome`/`toggle_canvas` (2-state toggles), `keymap_next`
   (cycle), `menu_bar_pin` (a 2-way pin toggle over a 3-state option), each beside explicit sets.
9. **A preset is a convenience over primitives — never the only door** to an option.
10. **Commands are the plugin/automation spine.** The test for "does this need a command?" is
    *"should a plugin ever be able to do this?"* — if yes, it's a command. Commands stay **nullary**
    today; parameterized set-value commands (`set_scrollbar("off")`) are an Effort-P concern. Keep
    set-value semantics clean so P can later collapse the N explicit-set commands into one
    parameterized command without breaking this contract.

---

## The one judgment call: menu vs palette-only placement

Everything above is mechanical. The single per-command judgment is *where a command surfaces*:

- **Menu (by category)** — the commands a word-processor user browses for: File, Edit
  (clipboard/undo), Format (transforms), View (toggles), Export, Settings; plus anything whose
  discoverability matters.
- **Palette-only** — motions & navigation, internal plumbing, keystroke-native ops, and the
  **set-per-state primitives** (their cycle command stands in for them in the menu).

The item-by-item application of this guideline across the command set is tracked as backlog A3b.

---

## Decision procedure (drop-in for any new feature or option)

1. **Runtime-changeable?** → it must be a command (law 2).
2. **Multi-state?** → set-per-state commands + one cycle (rule 8).
3. **Does a profile set it?** → the profile calls the same setter as the command (law 6).
4. **Browse-for-by-category, or not?** → menu vs palette-only (the one judgment).

Then, for free: it appears in the palette (law 3) with its live keymap hint (law 7), is reachable
by mouse and keyboard (law 5), and is controllable by a future plugin (rule 10).

Follow this and the ZEN/FULL-style orphaned-option gap cannot recur.

---

## Enforcement

The laws are backed by tests, not vigilance. As of the A3 effort the enforcing set is: the
palette-completeness invariant (law 3), the every-persisted-setting-has-a-command guard (law 2), and
the hints re-resolution + custom-bind + explicit-binding-preferred tests (law 7). New laws land with
a test. A spec/plan touching this surface states which of these it exercises.

## History

- 2026-07-06: the three-surface contract adopted (registry = truth; palette exhaustive; menu curated;
  live hints) — recorded in `docs/ux-backlog.md`'s governing-principle section.
- 2026-07-07 (A3 spec review): refined shape rule 8 — the menu representative is a *toggle or cycle*
  (not strictly "a cycle"), and it need not expose every state in the menu (the palette does). This
  reflects the existing `toggle_chrome`/`toggle_canvas` 2-state toggles and keeps `menu_bar_pin`
  compliant as menu_bar's representative.
- 2026-07-07: hardened into this contract after the ZEN/FULL density gap and the plugin-spine
  analysis (A3 brainstorm) — added law 2 (every option is a command), law 6 (shared setter), law 7's
  explicit-binding preference, and shape rules 8-10. This file is now the authoritative home; the
  backlog section points here.
