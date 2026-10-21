# skim-tab

Rust-native fuzzy tab completion for zsh. It replaces fzf-tab with the
[skim](https://github.com/skim-rs/skim) matcher and owns the whole pipeline:
candidate capture, fuzzy matching, preview, selection and path handling.

## Install

```sh
cargo install --git https://github.com/pleme-io/skim-tab
```

This installs `skim-tab` and the picker binaries `skim-history`, `skim-files`,
`skim-content`, `skim-cd`, `skim-fvim`, `skim-fco`, `skim-fkill` and `skim-kpod`.

## Usage

A zsh widget captures candidates with a `compadd` hook and pipes them to
`skim-tab`:

```
skim-tab --complete --compcap --command CMD --query Q
skim-tab --preview <manifest.json> <display_text>
skim-tab daemon [--socket PATH]
skim-tab daemon-status [--socket PATH]
```

The pickers cover history search (`skim-history`), file finding (`skim-files`),
content search (`skim-content`), smart `cd` (`skim-cd`), file opening
(`skim-fvim`), git checkout (`skim-fco`), process kill (`skim-fkill`) and
Kubernetes pod selection (`skim-kpod`).

## Configuration

`~/.config/skim-tab/skim-tab.yaml`. Set `SKIM_TAB_CONFIG` to use another path.

```yaml
completion:
  mode: direct
  single_auto_select: true
  preview:
    enable: true
    layout: "right:50%:wrap"
  picker:
    height: "40%"
  enrichment:
    lscolors: true
    descriptions: true
    k8s_live: true
tags:
  rules:
    - tag: danger
      weight: -1000
      contains: ["rm -rf", "--force"]
    - tag: git
      weight: 100
      command: [git]
```

Tag rules are an ordered priority list. The first match wins, and a negative
weight sinks an entry below untagged ones. Run
`skim-history --explain "git push --force"` to see which rule matched.

## Completion specs

Twelve specs are compiled in: kubectl, helm, flux, docker, git, nix, cargo,
npm, terraform, aws, gcloud and az. Specs in `~/.config/skim-tab/specs/`
override the built-ins, and specs in `.skim-tab/specs/` override both.

## Development

```sh
cargo test --workspace
cargo fmt --check
```

## License

MIT
