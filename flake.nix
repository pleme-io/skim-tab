{
  description = "skim-tab — Rust-native fuzzy completion bridge for zsh";

  # Canonical pleme-io Rust-tool consumer flake. substrate.rust.tool
  # pre-binds nixpkgs / crate2nix / flake-utils / fenix / devenv / gen
  # — every dependency the build kit needs — so a substrate bump
  # propagates fleet-wide without touching this file. toolName + repo
  # are read from the typed `flake_metadata.skim-tab` in
  # Cargo.build-spec.json.
  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.tool {
    src = ./.;
    module = {
      description = "skim-tab — Rust-native fuzzy completion bridge for zsh";

      # skim-tab has read a shikumi YAML at ~/.config/skim-tab/skim-tab.yaml
      # since it grew a config surface, but this flag was never set — so the
      # generated HM module shipped the BINARIES and no way to declare their
      # config. Every knob (completion.*, and now tags.*) was reachable only
      # by hand-writing a dotfile the fleet does not manage.
      #
      # substrate's module-trio already deploys that file from
      # `services.skim-tab.settings`; turning it on is the whole fix. No
      # hand-rolled module — the primitive existed, it just wasn't wired.
      withShikumiConfig = true;

      # Ship the tag rules the Ctrl-R picker was asked for, as DEFAULTS an
      # operator overrides via `services.skim-tab.settings` rather than as
      # something baked in. Order is the priority list (first match wins),
      # so `danger` is listed above `git` on purpose: `git push --force`
      # should sink, not float.
      shikumiDefaults = {
        tags = {
          enable = true;
          rules = [
            {
              tag = "danger";
              weight = -1000;
              contains = [ "rm -rf" "--force" "--hard" ];
            }
            {
              tag = "git";
              weight = 100;
              command = [ "git" "gst" "gco" "gp" "gd" "gl" "ga" "gc" ];
            }
            {
              tag = "nix";
              weight = 90;
              command = [ "nix" "nixos-rebuild" "darwin-rebuild" "nix-shell" ];
              prefix = [ "nix-" ];
            }
            {
              tag = "k8s";
              weight = 80;
              command = [ "kubectl" "k" "flux" "helm" "k9s" "banken" ];
            }
            {
              tag = "build";
              weight = 70;
              command = [ "cargo" "just" "make" "tend" ];
            }
          ];
        };
      };
    };
  };
}
