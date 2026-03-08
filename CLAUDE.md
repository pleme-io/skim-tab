# skim-tab — Rust-native fuzzy completion bridge for zsh

## Build & Test

```bash
cargo build          # compile
cargo test           # 11 unit tests
```

## Architecture

Drop-in replacement for `fzf` in fzf-tab, using skim as the fuzzy finder engine.
Fixes the `--expect` + `--print-query` output protocol mismatch between skim and fzf.

### The Problem

fzf-tab calls the fuzzy finder (normally fzf) with `--expect` and `--print-query`,
expecting this exact output format:

```
Line 1: query string        (from --print-query)
Line 2: matched expect key  (empty for Enter, key name for --expect match)
Line 3+: selected items
```

skim's `--expect` is deprecated in 3.x and doesn't output the empty key line for Enter,
causing fzf-tab to misparse the output and never insert the selected completion.

### The Fix

skim-tab:
1. Parses fzf-compatible CLI flags (`--expect`, `--print-query`, `--bind`, etc.)
2. Converts `--expect` keys to `--bind key:accept` (skim 3.x idiom)
3. Runs skim as a library via `Skim::run_with`
4. Formats output in the exact fzf protocol (always includes the key line)

### Module Map

| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI entry point, arg parsing, skim invocation, output formatting |

### Usage

```zsh
# In fzf-tab config:
zstyle ':fzf-tab:*' fzf-command skim-tab
```

## Consumers

- **blackmatter-shell** — fzf-tab plugin uses skim-tab as its fuzzy finder backend
