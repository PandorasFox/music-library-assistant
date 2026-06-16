{
  description = "Music Magic - music library management tool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ self, nixpkgs, flake-parts, rust-overlay }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" ];  # Other platforms untested

      flake = {
        lib = import ./nix/lib.nix;
        nixosModules.default = import ./nix/module.nix { inherit self; };
      };

      perSystem = { config, self', pkgs, system, ... }:
        let
          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" ];
          };

          nativeBuildInputs = with pkgs; [
            rustToolchain
            pkg-config
            cmake
          ];

          buildInputs = with pkgs; [
            # Audio encoding/decoding
            libopus

            # Chromaprint FFT backend
            fftwFloat

            # Memory allocator
            jemalloc

            # TLS for reqwest
            openssl
          ];
        in
        {
          _module.args.pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };

          devShells.default = pkgs.mkShell {
            inherit nativeBuildInputs buildInputs;

            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            JEMALLOC_SYS_WITH_LG_PAGE = "12";

            shellHook = ''
              echo "Music Magic dev shell"
              echo "Run: cargo build --release"
            '';
          };

          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "mm";
            version = "0.1.0";
            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            inherit nativeBuildInputs buildInputs;

            JEMALLOC_SYS_WITH_LG_PAGE = "12";

            meta = {
              description = "Music library management tool";
              homepage = "https://github.com/PandorasFox/music-magic";
              # license = TODO (add with pkgs.lib; when decided)
            };
          };

          # mm-tui is the user-facing app
          apps.default = {
            type = "app";
            program = "${self'.packages.default}/bin/mm-tui";
          };
        };
    };
}
