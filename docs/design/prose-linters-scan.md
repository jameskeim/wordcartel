# Prose-linter capability + client-needs scan (harper-ls / ltex-ls-plus / vale + vale-ls)

Live-fetched from official repos/docs + source files (2026-07-12). Grounding input for the
prose-linter / async design effort. Full detail; the highlights are summarized in the effort brief.

## A. Integration shape
- **harper-ls** — LSP server, stdio JSON-RPC, long-lived. Native **Rust** binary, fast, no external
  runtime; closest to bundleable. Cheap at rest.
- **ltex-ls-plus** (ltex-plus/ltex-ls-plus; the active fork — valentjn/ltex-ls is ARCHIVED, lineage
  only) — LSP server, stdio (or TCP). **Requires a JVM (Java 21+).** Release archives bundle a JRE →
  **~300 MB per platform**. No native-image/AOT option; still JVM. First-check latency **30 s – 2 min**
  (JVM boot + LanguageTool model load). The heavy outlier.
- **vale** — Go CLI, **one-shot** (run → JSON → exit). Trivially cheap, no residency.
- **vale-ls** (vale-cli/vale-ls; now **Rust**, not Go) — LSP server that **shells out to the `vale`
  CLI** per check (two external processes in the chain). Can **self-install** `vale` (`installVale:
  true`) — softens the binary-absent failure mode.

## B. LSP capabilities (per-diagnostic data)
- **publishDiagnostics source namespaces (distinct, non-colliding):** harper `source:"Harper"`,
  ltex `source:"LTeX"` (code = LanguageTool ruleId, e.g. PASSIVE_VOICE), vale-ls `source:"vale-ls"`
  (code = alert.check). → separate per-engine views map cleanly onto the wire data.
- **codeDescription.href (jump-to-rule-docs):** harper = **NONE** (never populated; even harper's own
  rule-reference page is "under construction"). ltex = populated → LanguageTool rule docs. vale-ls =
  populated **when the style author supplied a `link:`** (optional per rule).
- **codeAction / quick fixes:** harper — commands (HarperAddToUserDict/WSDict/FileDict, IgnoreLint,
  RecordLint). ltex — `_ltex.addToDictionary` / `_ltex.disableRules` / `_ltex.hideFalsePositives`,
  which are **CLIENT-HANDLED** (LTeX+ deliberately breaks the "server handles commands" norm — the
  client must implement the dictionary/settings mutation itself). vale-ls — one CodeAction per
  suggestion (multi), plus executeCommand `cli.sync`/`cli.compile` (NOT per-diagnostic dict/rule mgmt).
- **Multiple suggestions per diagnostic:** ltex YES (LanguageTool multi-candidate), vale YES
  (`suggest`/`replace` action arrays → N CodeActions), harper mostly single-canonical-fix.
- **hover:** harper — none (Obsidian etc. do hover client-side). ltex — explanation via message +
  href, no distinct hover. vale-ls — **TRAP: hover/completion exist but only for `.vale.ini` / style
  YAML files, NOT for prose alerts.** Do not assume "vale-ls hover = free rule explanations."
- **config:** harper PULL (workspace/configuration, JSON under "harper-ls"). ltex PULL + a custom
  `ltex/workspaceSpecificConfiguration` merge extension. vale — reads `.vale.ini` directly (no LSP
  config exchange); vale-ls config via initializationOptions.

## C. Settings surface size
- harper — moderate (per-rule bool table `lint_config`; 4-tier dictionary: user/workspace/file/static;
  dialect; severity; max_file_length 120 KB).
- ltex — **large** (~30 keys: per-language dictionary/disabledRules/enabledRules/hiddenFalsePositives
  with external file refs, markup tuning, LanguageTool internals, JVM heap knobs, per-rule-id severity).
- vale — **very large** (`.vale.ini` + a StylesPath YAML ecosystem: style packages, per-rule tuning,
  accept/reject vocabularies, per-format sections).

## D. Client-side views/hooks needed (per tool — the key deliverable)
- **(i) explanation/detail panel** — ESSENTIAL for all; harper from `message` text ALONE (no link);
  ltex/vale from message + `codeDescription.href` ("learn more").
- **(ii) multi-suggestion fix selector** — ESSENTIAL for ltex + vale; nice-to-have for harper.
- **(iii) dictionary / add-word mgmt** — ESSENTIAL; harper has 3 dict-tier commands; ltex's
  add-to-dictionary is **client-handled** (bigger lift); vale = vocab-file editing (no per-diagnostic
  LSP command).
- **(iv) rule/category enable-disable** — harper = settings-level (flat bool table, no runtime cmd);
  ltex = per-diagnostic **client-handled command** (`_ltex.disableRules`); vale = `.vale.ini`-editing.
- **(v) severity/category filter** — nice-to-have (harper global severity; ltex per-rule-id server-side;
  vale 3-level).
- **(vi) jump to rule docs URL** — free for ltex/vale (href); nothing to jump to for harper (no href).

## E. Lifecycle / resource posture
- harper — fast start, light, resident-cheap. Good "free at rest" fit.
- **ltex-ls-plus — POOR naive-always-on fit: 300 MB JVM, 30 s–2 min first check → structurally demands
  lazy-start + "warming up" status + idle-shutdown, never a blocking first-keystroke call** (matches the
  project's edge-triggered/swap-thrash-fix conventions).
- vale CLI — one-shot, no residency.
- vale-ls — Rust wrapper cheap; wrapped `vale` light; can auto-install the binary (best absent-binary UX).

## COMPARISON — where the client burden diverges
1. Explanation: harper = message-only (no link); ltex/vale = doc-link when present.
2. Fix-selection scales with suggestion model: ltex/vale need a real selector; harper less so.
3. Dictionary/rule mgmt is a genuine per-diagnostic **command** surface for harper + ltex (ltex's are
   CLIENT-HANDLED — the client implements the mutation); vale's is config-file editing.
4. vale-ls hover/completion = config-file-only trap (not prose).
5. Resource: vale one-shot cheap; harper/vale-ls light-resident; **ltex the heavy JVM outlier**.
6. Separate-per-engine-views well-supported by distinct `source` + non-colliding rule-id namespaces.

## Sources
Automattic/harper (backend.rs/config.rs/diagnostics.rs), writewithharper.com/docs; ltex-plus/ltex-ls-plus
+ vscode-ltex-plus (package.json), ltex-plus.github.io (faq/server-usage/installation/commands), gh api
releases; vale-cli/vale (internal/core/alert.go), vale.sh/docs (cli/actions); vale-cli/vale-ls
(server.rs/utils.rs/vale.rs), docs.vale.sh/guides/lsp.
</content>
