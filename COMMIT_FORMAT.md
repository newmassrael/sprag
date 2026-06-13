# Commit Message Format Guide (sprag)

## Structure

```
<type>(<scope>): <subject>

- <detail 1>
- <detail 2>
- <detail 3>
```

## Rules

### 1. Subject Line
- Format: `<type>(<scope>): <subject>` (scope optional but recommended)
- Types: `feat`, `fix`, `refactor`, `test`, `docs`, `build`, `chore`
- Subject: clear and concise; no period at the end; max 72 bytes

### 2. Scope (optional but recommended)

sprag-specific scopes:

| Scope | Domain |
|---|---|
| `core` | core multiplexer + scene-as-data substrate |
| `api` | plugin extension API contract (core <-> plugin) |
| `plugin` | plugin host + lifecycle + bundled plugins |
| `pty` | PTY driver (forkpty / Windows ConPTY, portable-pty) |
| `vt` | VT parser + cell-grid model |
| `mux` | multiplexing (split / resize / focus, tabs, sessions) |
| `render` | render dispatch (TUI cells / GPU window) |
| `tui` | TUI backend specifics |
| `gui` | GPU / GUI backend specifics |
| `mnemosyne` | atomic store / changelog mutations |
| `meta` | workspace metadata (mnemosyne.toml, .mcp.json, CLAUDE.md) |
| `hooks` | `.githooks/` (commit-msg, pre-commit, pre-push) |
| `build` | Cargo workspace, build.rs, rust-toolchain.toml |
| `docs` | README, comment-only fixes |
| `scaffold` | initial project setup / workspace skeleton |

### 3. Body
- One blank line after subject
- Bullet points (`- ` prefix) only — no prose lead paragraph
- Bullets must be **contiguous** — no blank line between bullets
- **1-3 items** — focus on key changes (fewer is better)
- The `commit-msg` hook enforces bullet-only + contiguity + the 1-3 cap
  (a prose body line, a blank line between bullets, or a 4th bullet is
  rejected, not just discouraged)
- **One bullet = one line, max 72 bytes total (incl. `- ` prefix)**
  - No continuation / indented wrap lines. Rewrite tighter or split.
  - Verify with: `git log -1 --format=%B | awk '{print length, $0}'`
- Be specific and technical
- Reference Mnemosyne sections in `§N.M` form once the spec exists
- Reference Mnemosyne rounds as `R<N>` (e.g., R1, R2)

### 4. Style
- **English only** — subject and body in English so the log stays
  accessible to every collaborator. ASCII printable (U+0020 to U+007E)
  plus the typographic whitelist below are the only permitted code
  points; any other character (Hangul, Kana, CJK, Cyrillic, Greek,
  etc.) is rejected by the commit-msg hook.
  - Typographic whitelist: `§` (U+00A7), `–` (U+2013), `—` (U+2014),
    `•` (U+2022), `…` (U+2026), `→` (U+2192).
  - Korean round summaries / narrative belong in in-tree docs or the
    `memory/` store, never in the commit message.
- **No emojis** (Unicode pictograph ranges U+1F300-U+1FAFF and
  U+1F1E6-U+1F1FF are rejected)
- **No "Generated with Claude Code"**
- **No "Co-Authored-By" tags**
- Professional and technical tone
- Focus on "what" and "why", not "how"
- Quantify Mnemosyne validate deltas where possible
  (e.g., `entries 0 → 1`, `sections 0 → 2`, `T3 warn 0 → 0`)

## Type Guidelines

| Type | When to Use | Examples |
|------|-------------|----------|
| `feat` | New capability, plugin, API surface, core module | PTY driver first impl; plugin host MVP |
| `fix` | Correctness fix, supersede a wrong design call | fix cell-grid cursor off-by-one |
| `refactor` | Structural change without semantic shift | extract VT parser into its own crate |
| `test` | Unit / integration test addition | add single-pane I/O round-trip test |
| `docs` | Comment-only fix, README, CLAUDE.md | clarify scene-as-data invariant in README |
| `build` | Cargo workspace, build.rs, toolchain, hooks | pin rust-toolchain; add pre-push gate |
| `chore` | Project structure, gitignore, housekeeping | scaffold initial workspace |

## Examples

### Good: Initial scaffold (chore)
```
chore(scaffold): initialize sprag workspace + Mnemosyne + hooks

- README: plugin-platform purpose + scene-as-data invariant + scope
- Mnemosyne store-direct (mnemosyne.toml + .mcp.json); validate clean
- COMMIT_FORMAT.md + .githooks (commit-msg/pre-commit/pre-push) gates
```

### Good: Spec axis close (feat with mnemosyne scope)
```
feat(mnemosyne): R1 §1 vision + §2 scene-as-data invariant

- §1 plugin-platform identity; §2 no-pixels introspection invariant
- entries 0 → 1; sections 0 → 2; T1 orphan 0; validate clean
- core/plugin boundary recorded as the primary design axis
```

### Good: Core module first-impl (feat with pty scope)
```
feat(pty): single-pane PTY driver over portable-pty

- spawn + read/write loop; forkpty (Linux) / ConPTY (Windows) abstracted
- cell-grid not yet wired; raw bytes surfaced for the VT parser stage
- 3 unit tests: spawn, echo round-trip, resize
```

### Good: Hook installation (build)
```
build(hooks): commit-msg + pre-commit/pre-push validate gates

- commit-msg enforces COMMIT_FORMAT.md (bullets, English, no attribution)
- pre-commit/pre-push run mnemosyne-cli validate-workspace
- core.hooksPath = .githooks (one-time per clone)
```

### Bad: Prose body (no bullets)
```
chore: set up project

Added the readme and some config files for the new project.
```
**Problem**: body is a prose paragraph. Rule is `- ` bullets only.

### Bad: Too many bullets
```
feat(core): multiplexer foundation

- pane model
- split logic
- focus tracking
- tab bar
- session store
```
**Problem**: 5 items — condense to 1-3 key changes.

## Domain-Specific Guidelines

### Mnemosyne workspace mutations (`feat(mnemosyne)` / `fix(mnemosyne)`)
- Reference the primitive used (`add-section`, `set-section-intent`, ...)
- Cite atomic ledger delta: `entries N → M`, `sections N → M`
- Cite validate metrics: `T1=0`, `T3 warn N → M`
- The round entry is the close marker

### Core / plugin-API decisions (`feat(core)` / `feat(api)`)
- Crate + module path
- Public API surface introduced (the core <-> plugin contract)
- Test pass count
- Note the scene-as-data invariant is upheld (no opaque pixels)

## Key Points
- 1-3 bullets (fewer when sufficient)
- No emojis, no attribution tags, English only
- Quantify Mnemosyne deltas (`entries`, `sections`, `T1`, `T3 warn`)
- Keep the scene-as-data invariant visible in core / plugin commits
