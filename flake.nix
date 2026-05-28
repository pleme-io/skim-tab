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
    };
  };
}
