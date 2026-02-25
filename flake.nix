{
  description = "wsh - The Web Shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    llm-agents.url = "github:numtide/llm-agents.nix";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, llm-agents, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default;
      in
      {
        packages.default = let
          webFrontend = pkgs.stdenvNoCC.mkDerivation {
            pname = "wsh-web";
            version = "0.1.0";
            src = ./.;
            nativeBuildInputs = [ pkgs.bun ];

            # FOD: allows network access; hash must be updated when web/ changes
            outputHashAlgo = "sha256";
            outputHashMode = "recursive";
            outputHash = "sha256-4AgZw+WAxDoTqJatqvEjzBMm4uRSz9bLwZL8gStblo4=";

            buildPhase = ''
              export HOME=$TMPDIR
              cd web
              bun install --frozen-lockfile
              bun run --bun node_modules/.bin/tsc
              bun run --bun node_modules/.bin/vite build
            '';

            installPhase = ''
              cp -r ../web-dist $out
            '';
          };
        in pkgs.rustPlatform.buildRustPackage {
          pname = "wsh";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.pkg-config ];

          preBuild = ''
            cp -r ${webFrontend} web-dist
          '';

          WSH_SKIP_WEB_BUILD = "1";

          # Tests that spawn a PTY need a real shell, which isn't
          # available in the Nix build sandbox.
          doCheck = false;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config
            curl
            jq
            websocat
            bun
          ] ++ [
            llm-agents.packages.${system}.agent-browser
          ];

          # nix develop overwrites $SHELL with stdenv's readline-less bash,
          # which breaks prompt escapes and any tool that spawns $SHELL
          # interactively (including wsh). Restore the user's login shell.
          # Upstream: https://github.com/NixOS/nix/issues/12008
          # Use a separate server instance for local development so we
          # don't collide with any system-wide wsh server.
          WSH_SERVER_NAME = "dev";

          shellHook = ''
            export SHELL="$(getent passwd "$USER" | cut -d: -f7)"
          '';
        };
      }
    );
}
