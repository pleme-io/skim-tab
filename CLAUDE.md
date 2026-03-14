# skim-tab — Rust-native fuzzy tab completion for zsh

## Build & Test

```bash
cargo build          # compile
cargo test           # unit tests
cargo check          # type-check only
```

## Architecture

Standalone fuzzy completion engine that replaces fzf-tab entirely.
Not a bridge — skim-tab owns the full pipeline: candidate capture,
fuzzy matching, preview, selection, and path handling.

### Pipeline

```
zsh _main_complete → compadd hook captures candidates
                   → skim-tab --complete (Rust) fuzzy picks
                   → zle _skim-tab-apply inserts selection
```

### Three-Path Widget (blackmatter-shell)

```
Tab pressed
  ├── Path A: LBUFFER ends with / → native zsh (IPREFIX splitting)
  ├── Path B: candidates captured → skim-tab Rust picker → apply
  └── Path C: no candidates → redisplay (no-op)
```

### Module Map

| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI dispatch: --complete, --preview, --history, --files, etc. |
| `src/complete.rs` | Core completion: compcap parsing, colorization, skim runner, selection |
| `src/config.rs` | shikumi-based YAML config with feature flags |
| `src/descent.rs` | In-picker directory descent (optional, gated by config) |
| `src/history.rs` | Ctrl+R history search with lossy UTF-8 |
| `src/preview.rs` | Preview pane rendering for candidates |
| `src/k8s.rs` | Kubernetes enrichment (resource counts, namespace info) |
| `src/cd.rs` | Smart cd widget (zoxide integration) |
| `src/files.rs` | Ctrl+T file finder |
| `src/content.rs` | Ctrl+F content search |
| `src/fco.rs` | Git checkout fuzzy finder |
| `src/fkill.rs` | Process killer |
| `src/fvim.rs` | File opener |
| `src/kpod.rs` | K8s pod selector |
| `src/lib.rs` | Shared: Nord colors, skim builder presets, ANSI helpers |

### Configuration

`~/.config/skim-tab/skim-tab.yaml` (shikumi discovery, env override `SKIM_TAB_CONFIG`)

```yaml
completion:
  mode: direct                    # direct | service | hybrid
  single_auto_select: true        # skip picker for 1 candidate
  in_picker_descent: false        # readdir loop inside picker
  preview:
    enable: true
    directories: true
    files: true
    max_lines: 20
    layout: "right:50%:wrap"
  picker:
    height: "40%"
    cycle: true
    no_sort: true
    group_colors: true
  dir_handling:
    append_slash: true            # / on directory words
    skip_trailing_space: true     # no space after dirs
  enrichment:
    lscolors: true                # LS_COLORS file coloring
    descriptions: true            # command/subcommand descriptions
    k8s_live: true                # live kubectl resource counts
```

### Key Design Decisions

- **Rust-first**: All intelligence (dir detection, path stat, enrichment, colorization) in Rust. Zsh is a thin wrapper.
- **Config-gated features**: Every feature has a flag. Defaults match current behavior. New features opt-in.
- **Native fallback for path descent**: When LBUFFER ends with /, bypass skim and let zsh's `_path_files` handle IPREFIX splitting. This is the only way to get correct multi-level cd descent without reimplementing zsh's path completer.
- **compadd -Q**: Required for the fzf-tab protocol but prevents zsh from managing directory suffixes. We compensate by appending / in Rust and signaling no-space to zsh.

## Consumers

- **blackmatter-shell** — skim-tab-complete plugin (init.zsh widget + keybindings)
- **blackmatter-pleme** — Nix home-manager module deploys config

## Roadmap

See `docs/ROADMAP.md` for the phased feature plan.
