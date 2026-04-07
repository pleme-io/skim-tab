# skim-tab — Rust-native fuzzy tab completion for zsh

## Build & Test

```bash
cargo build          # compile
cargo test           # 453+ tests across all binaries
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
| `src/specs.rs` | `DescriptionProvider` trait (with `is_k8s_command`), `SpecRegistry` (12 built-in YAML specs), `CompletionSpec` serde types |
| `src/config.rs` | shikumi-based YAML config with feature flags |
| `src/context.rs` | Context introspection helpers |
| `src/history_db.rs` | `HistoryStore` trait, `HistoryDb` (SQLite), `MemHistoryStore` (test), `frecency_score()` pure fn |
| `src/descent.rs` | In-picker directory descent (optional, gated by config) |
| `src/history.rs` | Ctrl+R history search with lossy UTF-8 |
| `src/preview.rs` | Preview pane rendering for candidates |
| `src/k8s.rs` | `KubeconfigLoader`/`KubectlRunner` traits, `KubeContext`, resource counts, namespace enrichment |
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

### YAML Completion Specs

12 built-in specs compiled into the binary via `BUILTIN_SPECS`:

| Spec | Commands | Entries |
|------|----------|---------|
| `kubectl.yaml` | kubectl, kubecolor, k | 35 subcommands + 76 resource types |
| `helm.yaml` | helm | 27 subcommands + 5 show-subcommands |
| `flux.yaml` | flux | 22 subcommands + 30 resource types |
| `docker.yaml` | docker, podman | 13 subcommands |
| `git.yaml` | git | 22 subcommands |
| `nix.yaml` | nix | 17 subcommands (with nesting) |
| `cargo.yaml` | cargo | 18 subcommands |
| `npm.yaml` | npm, pnpm, yarn | 12 subcommands |
| `terraform.yaml` | terraform, tofu | 12 subcommands |
| `aws.yaml` | aws | 26 services |
| `gcloud.yaml` | gcloud | 23 services |
| `az.yaml` | az | 24 services |

User specs from `~/.config/skim-tab/specs/` override built-ins. Project specs from `.skim-tab/specs/` override both.

Auto-generated specs can be produced by **completion-forge** from OpenAPI specs.

### Key Design Decisions

- **Rust-first**: All intelligence (dir detection, path stat, enrichment, colorization) in Rust. Zsh is a thin wrapper.
- **Config-gated features**: Every feature has a flag. Defaults match current behavior. New features opt-in.
- **YAML-driven descriptions**: All command descriptions live in YAML specs (no hardcoded registries). `DescriptionProvider` trait abstracts lookup.
- **Trait-based K8s detection**: `is_k8s_command()` on `DescriptionProvider` checks spec icon for K8s helm wheel — prevents aws/gcloud/az from triggering kubectl calls.
- **Zero-copy lookups**: `DescriptionProvider::lookup()` returns `(&str, &str)` refs; `api_type_to_plural()` returns `Cow<'static, str>`.
- **Native fallback for path descent**: When LBUFFER ends with /, bypass skim and let zsh's `_path_files` handle IPREFIX splitting. This is the only way to get correct multi-level cd descent without reimplementing zsh's path completer.
- **compadd -Q**: Required for the fzf-tab protocol but prevents zsh from managing directory suffixes. We compensate by appending / in Rust and signaling no-space to zsh.

## Consumers

- **blackmatter-shell** — skim-tab-complete plugin (init.zsh widget + keybindings)
- **blackmatter-pleme** — Nix home-manager module deploys config

## Roadmap

See `docs/ROADMAP.md` for the phased feature plan.
