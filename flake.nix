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
        rustToolchainDarwin = pkgs.rust-bin.stable.latest.default.override {
          targets = [
            "x86_64-apple-darwin"
            "aarch64-apple-darwin"
          ];
        };
      in
      {
        packages = let
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

          macOsSdk = pkgs.fetchurl {
            url = "https://github.com/joseluisq/macosx-sdks/releases/download/14.5/MacOSX14.5.sdk.tar.xz";
            hash = "sha256-bhRiddGfAn+qLoNU2l4CZ1E6vwE7jxatZaIxZTorHF0=";
          };

          # FOD: hash must be updated when Cargo.lock changes
          cargoVendor = pkgs.rustPlatform.fetchCargoVendor {
            src = ./.;
            hash = "sha256-9yCS4AtmUq2Gh+ZsFiDCz2/wzcgjEgbuGSHvvf/02Gk=";
          };

          mkWsh = crossPkgs: crossPkgs.rustPlatform.buildRustPackage {
            pname = "wsh";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ crossPkgs.pkg-config ];

            preBuild = ''
              cp -r ${webFrontend} web-dist
            '';

            WSH_SKIP_WEB_BUILD = "1";

            # Tests that spawn a PTY need a real shell, which isn't
            # available in the Nix build sandbox.
            doCheck = false;
          };

          mkWshDarwin = target: pkgs.stdenv.mkDerivation {
            pname = "wsh";
            version = "0.1.0";
            src = ./.;

            nativeBuildInputs = [
              rustToolchainDarwin
              pkgs.zig
              pkgs.cargo-zigbuild
            ];

            buildPhase = ''
              export HOME=$TMPDIR

              # Vendor cargo dependencies
              mkdir -p .cargo
              cat > .cargo/config.toml <<CARGOEOF
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "${cargoVendor}"
CARGOEOF

              # Untar macOS SDK
              mkdir -p $TMPDIR/sdk
              tar xf ${macOsSdk} -C $TMPDIR/sdk
              export SDKROOT=$TMPDIR/sdk/MacOSX14.5.sdk
              export MACOSX_DEPLOYMENT_TARGET=11.0

              # Framework and library linking
              export LIBRARY_PATH=$SDKROOT/usr/lib
              export RUSTFLAGS="-L framework=$SDKROOT/System/Library/Frameworks"

              # Copy pre-built web frontend
              cp -r ${webFrontend} web-dist
              export WSH_SKIP_WEB_BUILD=1

              cargo zigbuild --release --target ${target}
            '';

            installPhase = ''
              mkdir -p $out/bin
              cp target/${target}/release/wsh $out/bin/wsh
            '';
          };
        in {
          default = mkWsh pkgs;
          wsh-x86_64-linux-musl = mkWsh pkgs.pkgsCross.musl64.pkgsStatic;
          wsh-aarch64-linux-musl = mkWsh pkgs.pkgsCross.aarch64-multiplatform-musl.pkgsStatic;
          wsh-x86_64-apple-darwin = mkWshDarwin "x86_64-apple-darwin";
          wsh-aarch64-apple-darwin = mkWshDarwin "aarch64-apple-darwin";
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config
            curl
            jq
            websocat
            bun
            qemu
            zig
            cargo-zigbuild
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
