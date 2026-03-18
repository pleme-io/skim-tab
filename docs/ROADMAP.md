# skim-tab Roadmap

Phased feature plan. Each round is self-contained — ship and stabilize before
moving to the next. All features gated behind config flags.

---

## Round 1: Foundation Polish (current)

**Status: SHIPPED**

What we have today:

- [x] Fuzzy completion via skim (replaces fzf-tab)
- [x] Nord-themed colorization (lscolors + ANSI)
- [x] K8s enrichment (live resource counts, namespace info)
- [x] Command/subcommand description registry (kubectl, helm, flux, etc.)
- [x] Preview pane (command-aware: files, dirs, k8s resources)
- [x] Directory detection via Rust stat — append `/`, signal no-space
- [x] Native zsh fallback for path descent (LBUFFER ends with /)
- [x] In-picker descent (optional, config-gated)
- [x] Lossy UTF-8 history search
- [x] shikumi config with full feature flag system
- [x] Single-candidate auto-select

---

## Round 2: Picker UX Enhancements

**Status: SHIPPED**

### 2a. Continuous completion trigger
- **Config**: `picker.continuous_trigger: "/"`
- **What**: Pressing `/` inside the skim picker on a directory candidate
  triggers immediate descent (readdir + new picker) without returning to zsh.
- **Why**: fzf-tab's #1 UX feature. Users can type `cd src/` and keep
  navigating without leaving the picker.
- **Implementation**: Custom skim keybinding (`--bind /: ...`) that checks
  if the highlighted item is a directory, reads it, and replaces the item list.
  Skim doesn't natively support this — we'd need to use `accept` + loop,
  or fork skim to add a `reload` action.

### 2b. Group switching (F1/F2)
- **Config**: `picker.group_switching: true`
- **What**: F1/F2 cycle through completion groups (e.g., files vs flags vs
  subcommands). skim header updates to show the active group.
- **Why**: When completions mix types (flags + files + commands), filtering
  by group is faster than scrolling.
- **Implementation**: Pre-sort candidates by group. F1/F2 filters the list
  to the next/previous group. Requires skim keybinding + item filtering.

### 2c. Smart menu threshold
- **Config**: `picker.min_candidates: 2`
- **What**: Don't show the picker at all below the threshold — just insert.
  At exactly 1, auto-select (already have this). At 2+, show picker.
  Setting to 1 effectively disables the picker for unambiguous completions.
- **Why**: Users complained about fzf-tab showing a menu for 2 items when
  Tab-Tab would be faster.
- **Implementation**: Check candidate count against threshold before launching skim.

---

## Round 3: Preview System

**Goal**: Rich, command-aware preview that shows useful context.

### 3a. Directory content preview
- **Config**: `preview.directories: true`
- **What**: When highlighting a directory in the picker, the preview pane
  shows its contents (via eza with icons, or ls fallback).
- **Implementation**: skim `--preview` command that detects directory candidates
  and runs `eza -la --icons --color=always {realdir}{word}`.

### 3b. File content preview
- **Config**: `preview.files: true`
- **What**: When highlighting a file, show contents via bat (syntax highlighted)
  or cat. Binary files show hexyl or file type info.
- **Implementation**: Preview command detects file type and dispatches:
  text → `bat --color=always --style=numbers`, binary → `file` + `hexyl`.

### 3c. K8s resource preview
- **Config**: `preview.k8s: true`
- **What**: When completing kubectl resources, preview shows `kubectl describe`
  output for the highlighted resource.
- **Implementation**: Preview command parses the buffer to detect kubectl context,
  runs `kubectl describe {type} {name}` with timeout.

### 3d. Git preview
- **Config**: `preview.git: true`
- **What**: When completing git branches, show `git log --oneline -10 {branch}`.
  For git files, show `git diff {file}`.

---

## Round 4: Context Intelligence

**Goal**: Completions that understand where you are and what you're doing.

### 4a. Project-aware completions
- **Config**: `context.project_detection: true`
- **What**: Detect project type (Rust, Node, Python, Go, Nix) from the CWD
  and boost relevant completions. E.g., in a Cargo project, `cargo` subcommands
  get richer descriptions and `target/` is deprioritized.
- **Implementation**: Scan for Cargo.toml, package.json, go.mod, flake.nix, etc.
  Set context flags that enrichment functions read.

### 4b. History-weighted ranking
- **Config**: `context.history_boost: true`
- **What**: Boost candidates that the user has selected before in this directory
  or for this command. Uses a small SQLite database (like atuin).
- **Why**: After the 3rd time you `cd` to the same dir, it should be at the top.
- **Implementation**: `~/.local/share/skim-tab/selections.db` — record
  (command, cwd, selected_word, timestamp). Query on each completion to
  sort candidates by frequency.

### 4c. Frecency scoring
- **Config**: `context.frecency: true`
- **What**: Combine frequency + recency (like zoxide's algorithm) for candidate
  ranking. Recent selections rank higher than old frequent ones.
- **Implementation**: Extend the selections.db with a frecency score column.
  Score = frequency * recency_decay.

---

## Round 5: Multi-Shell Foundation

**Status: SHIPPED**

### 5a. YAML completion specs
- **Config**: `specs.enable: true`, `specs.dirs: ["~/.config/skim-tab/specs"]`
- [x] 12 built-in YAML specs compiled into binary (kubectl, helm, flux, docker,
  git, nix, cargo, npm, terraform, aws, gcloud, az)
- [x] User specs from `~/.config/skim-tab/specs/` override built-ins
- [x] Project specs from `.skim-tab/specs/` override both
- [x] `DescriptionProvider` trait abstracts lookup — `SpecRegistry` implements it
- [x] `completion-forge` can auto-generate specs from OpenAPI specs

### 5b. Spec-based enrichment
- [x] YAML specs are the sole source of descriptions and icons
- [x] Old hardcoded TOOL_REGISTRY fully removed
- [x] `is_k8s_command()` explicit match prevents false K8s enrichment

---

## Round 6: Advanced UX

**Goal**: Polish features that differentiate from all competitors.

### 6a. Tmux/multiplexer popup mode
- **Config**: `picker.popup: true`
- **What**: When inside tmux, render the picker as a floating popup window
  instead of inline. Centers on screen, looks like an IDE command palette.
- **Implementation**: Use `tmux display-popup` to spawn skim in a popup.
  Requires detecting tmux via `$TMUX` env var.

### 6b. Multi-select with batch insert
- **Config**: `picker.multi_select: true`
- **What**: Tab to toggle multiple selections in the picker. All selected
  items are inserted space-separated. Useful for `git add`, `rm`, etc.
- **Implementation**: skim already supports `--multi`. Parse multiple
  selections in the apply phase.

### 6c. Accept-and-execute
- **Config**: `picker.accept_execute_key: "ctrl-x"`
- **What**: A keybinding that selects AND immediately executes the command
  (like pressing Enter twice). Useful for `cd` where you always want to
  execute after completing.

### 6d. Inline completion (no picker)
- **Config**: `completion.inline_mode: true`
- **What**: Like zsh-autocomplete: show completions inline below the prompt
  as you type, without waiting for Tab. Tab confirms the top match.
- **Why**: The "Apple Intelligence"-style UX — instant feedback.
- **Implementation**: This is a major architecture change. Requires async
  completion generation and a custom ZLE widget that renders below the prompt
  on every keystroke (precmd hook + POSTDISPLAY).

---

## Implementation Priority

| Round | Effort | Impact | Status |
|-------|--------|--------|--------|
| R1 Foundation | Done | High | Shipped |
| R2 Picker UX | Done | High | Shipped |
| R3a Dir preview | Done | Medium | Shipped |
| R3b File preview | Done | Medium | Shipped |
| R4b History-weighted | Done | High | Shipped |
| R4c Frecency | Done | High | Shipped |
| R5a YAML specs | Done | High | Shipped |
| R5b Spec-based enrichment | Done | High | Shipped |
| R3c K8s preview | Medium | Medium | Later |
| R4a Project-aware | Medium | Medium | Later |
| R6a Tmux popup | Medium | Medium | Future |
| R6d Inline mode | Very Large | Very High | Future |
