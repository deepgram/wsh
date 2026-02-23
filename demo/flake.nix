{
  description = "wsh demo recording tools";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Recording (Wayland/Sway)
            wf-recorder

            # Video processing
            ffmpeg

            # High-quality GIF encoding
            gifski

            # Used by demo scripts
            curl
            jq
            bc
          ];
        };
      }
    );
}
