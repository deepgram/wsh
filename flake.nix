{
  description = "wsh - The Web Shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default;
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config
            curl
            jq
            websocat
            bun
          ];

          # nix develop overwrites $SHELL with stdenv's readline-less bash,
          # which breaks prompt escapes and any tool that spawns $SHELL
          # interactively (including wsh). Restore the user's login shell.
          # Upstream: https://github.com/NixOS/nix/issues/12008
          shellHook = ''
            export SHELL="$(getent passwd "$USER" | cut -d: -f7)"
          '';
        };
      }
    );
}
