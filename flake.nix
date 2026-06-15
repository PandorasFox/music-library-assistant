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
        nixosModules.default = { config, lib, pkgs, ... }:
          let
            cfg = config.services.music-magic;
            pkg = self.packages.${pkgs.system}.default;
          in {
            options.services.music-magic = {
              enable = lib.mkEnableOption "Music Magic server";

              storageRoot = lib.mkOption {
                type = lib.types.path;
                description = "Path to the music corpus directory";
                example = "/srv/media/audio/Music";
              };

              librariesRoot = lib.mkOption {
                type = lib.types.nullOr lib.types.path;
                default = null;
                description = "Path for library deployments (defaults to storageRoot/../libraries)";
              };

              stashRoot = lib.mkOption {
                type = lib.types.nullOr lib.types.path;
                default = null;
                description = "Path for stashed files (defaults to storageRoot/../stash)";
              };

              serviceUser = lib.mkOption {
                type = lib.types.str;
                default = "music-magic";
                description = "System user to run Music Magic service as";
              };

              serviceGroup = lib.mkOption {
                type = lib.types.str;
                default = "music-magic";
                description = "System group to run Music Magic service as";
              };

              dataDir = lib.mkOption {
                type = lib.types.path;
                default = "/var/lib/mm";
                description = "Directory for database and state (/var/lib/mm by default)";
              };

              adminUser = lib.mkOption {
                type = lib.types.str;
                default = "admin";
                description = "Username for initial admin account in Music Magic";
              };

              adminPasswordFile = lib.mkOption {
                type = lib.types.nullOr lib.types.path;
                default = null;
                description = ''
                  Path to file containing the initial admin password.
                  Use with sops-nix or agenix for secure secret management.
                  If null, first-time setup must be completed interactively via mm-tui.
                '';
                example = "/run/secrets/mm-admin-password";
              };

              runtimeDir = lib.mkOption {
                type = lib.types.path;
                default = "/run/mm";
                description = ''
                  Directory for the Unix socket (mm.sock).
                  Users in serviceGroup can connect via mm-tui by setting:
                    XDG_RUNTIME_DIR=/run/mm mm-tui
                '';
              };
            };

            config = lib.mkIf cfg.enable {
              users.users.${cfg.serviceUser} = {
                isSystemUser = true;
                group = cfg.serviceGroup;
                home = cfg.dataDir;
                createHome = true;
              };

              users.groups.${cfg.serviceGroup} = {};

              systemd.services.music-magic = {
                description = "Music Magic Server";
                after = [ "network.target" ];
                wantedBy = [ "multi-user.target" ];

                environment = {
                  # XDG bases are parent of dataDir; app appends /mm, so:
                  #   config: /var/lib/mm/config.kdl
                  #   database: /var/lib/mm/mm.db
                  XDG_CONFIG_HOME = builtins.dirOf cfg.dataDir;
                  XDG_DATA_HOME = builtins.dirOf cfg.dataDir;
                  XDG_RUNTIME_DIR = cfg.runtimeDir;
                };

                serviceConfig = {
                  Type = "simple";
                  User = cfg.serviceUser;
                  Group = cfg.serviceGroup;
                  ExecStart = "${pkg}/bin/mm";
                  Restart = "on-failure";
                  RestartSec = "5s";

                  # Runtime directory for socket (created automatically on service start)
                  RuntimeDirectory = baseNameOf cfg.runtimeDir;
                  RuntimeDirectoryMode = "0770";

                  # Hardening
                  NoNewPrivileges = true;
                  ProtectSystem = "full";  # Only protects /usr, /boot, /efi; /var and /srv stay writable
                  ProtectHome = true;
                  PrivateTmp = true;
                  # NOTE: Do NOT use ReadWritePaths - it creates a mount namespace with
                  # separate bind mounts that cause EXDEV on rename() between paths.
                };

                preStart = ''
                  # Generate config.kdl at $XDG_CONFIG_HOME/mm/config.kdl = ${cfg.dataDir}/config.kdl
                  mkdir -p ${cfg.dataDir}
                  cat > ${cfg.dataDir}/config.kdl <<EOF
                  storage-root "${cfg.storageRoot}"
                  ${lib.optionalString (cfg.librariesRoot != null) ''libraries-root "${cfg.librariesRoot}"''}
                  ${lib.optionalString (cfg.stashRoot != null) ''stash-root "${cfg.stashRoot}"''}
                  EOF

                  ${lib.optionalString (cfg.adminPasswordFile != null) ''
                    # Initialize database with admin user if it doesn't exist
                    if [ ! -f ${cfg.dataDir}/mm.db ]; then
                      ${pkg}/bin/mm --init-user ${cfg.adminUser} --password-file ${cfg.adminPasswordFile}
                    fi
                  ''}
                '';
              };
            };
          };
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
