{
  description = "Rust dev flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    naersk.url = "github:nix-community/naersk";
    flake-utils.url = "github:numtide/flake-utils";
    fenix.url = "github:nix-community/fenix";
  };
  outputs =
    {
      self,
      flake-utils,
      naersk,
      nixpkgs,
      fenix,
    }:
    (flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (import nixpkgs) {
          inherit system;
          overlays = [ fenix.overlays.default ];
        };
        naersk' = pkgs.callPackage naersk { };

        # Programs tansu runs for previews. All are optional at runtime: without one, the
        # preview it provides falls back to plain metadata.
        previewTools = with pkgs; [
          bat # syntax highlighting
          ffmpeg-headless # ffprobe (video and audio details) and ffmpeg (frames, covers)
          ffmpegthumbnailer # video frames
          poppler-utils # pdftoppm (PDF pages) and pdfinfo (PDF details)
        ];

        tansu-unwrapped = naersk'.buildPackage {
          src = ./.;
          nativeBuildInputs = with pkgs; [
            pkg-config
            sqlite
          ];
          buildInputs = with pkgs; [ openssl ];
        };

        # tansu with the preview tools on PATH. They are appended, so programs the user
        # already has on PATH take precedence.
        tansu = pkgs.symlinkJoin {
          name = "tansu-${tansu-unwrapped.version}";
          paths = [ tansu-unwrapped ];
          nativeBuildInputs = [ pkgs.makeWrapper ];
          postBuild = ''
            wrapProgram $out/bin/tansu --suffix PATH : ${pkgs.lib.makeBinPath previewTools}
          '';
          meta = tansu-unwrapped.meta // {
            mainProgram = "tansu";
          };
        };
      in
      rec {
        defaultPackage = tansu;

        packages = {
          default = tansu;
          inherit tansu tansu-unwrapped;
        };

        devShell = pkgs.mkShell {
          buildInputs = with pkgs; [
            (pkgs.fenix.stable.withComponents [
              "rustc"
              "cargo"
              "rust-analyzer"
              "clippy"
              "rustfmt"
            ])
            sqlite
            cargo-audit
          ]
          ++ previewTools;
          nativeBuildInputs = with pkgs; [
            (openssl.override { static = true; })
            pkg-config
          ];
          shellHook = ''
            export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:${pkgs.lib.makeLibraryPath [ pkgs.sqlite ]}"
          '';
        };

        devShells.default = devShell;
      }
    ))
    // {
      homeManagerModules.default = import ./modules/home-manager.nix { inherit self; };
    };
}
