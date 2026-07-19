# NixOS module for Music Magic
{ self }:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.music-magic;
  pkg = self.packages.${pkgs.system}.default;
  inherit (self.lib) serializePathSchema;

  pathSchemaType = lib.types.nullOr (lib.types.either
    lib.types.str
    (lib.types.listOf lib.types.unspecified)
  );

  # Group libraries by src path so each unique src becomes one dir entry in dirs.kdl.
  # (MM's resolve_source_config picks one dir per path — duplicates lose libraries.)
  srcPaths = lib.unique (map (opts: opts.src) (lib.attrValues cfg.libraries));
  dirsKdl = lib.concatMapStringsSep "\n" (srcPath:
    let
      libsForSrc = lib.filterAttrs (_: opts: opts.src == srcPath) cfg.libraries;
      libNames = lib.attrNames libsForSrc;
      firstOpts = lib.head (lib.attrValues libsForSrc);
      serialized = serializePathSchema firstOpts.pathSchema;
    in ''
      dir "${srcPath}" {
          library ${lib.concatMapStringsSep " " (l: ''"${l}"'') libNames}
      ${lib.optionalString (serialized != null) ''    path-schema "${serialized}"''}
      }
    '') srcPaths;
in {
  options.services.music-magic = {
    enable = lib.mkEnableOption "Music Magic server";

    srcRoot = lib.mkOption {
      type = lib.types.path;
      description = "Corpus directory (source of truth for all audio files)";
      example = "/srv/media/audio/inbox";
    };

    librariesDir = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Root directory under which named library subdirectories are created";
      example = "/srv/media/audio";
    };

    stashDir = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Directory for stashed files (dupes, transcodes, etc.)";
      example = "/srv/media/audio/stash";
    };

    serviceUser = lib.mkOption {
      type = lib.types.str;
      default = "music-magic";
      description = "System user to run the Music Magic service as";
    };

    serviceGroup = lib.mkOption {
      type = lib.types.str;
      default = "music-magic";
      description = "System group to run the Music Magic service as";
    };

    dataDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/mm";
      description = "Directory for database and runtime state";
    };

    adminUser = lib.mkOption {
      type = lib.types.str;
      default = "admin";
      description = "Username for the initial admin account";
    };

    adminPasswordFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        Path to a file containing the initial admin password.
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

    autoDeploy = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Automatically deploy corpus changes to libraries";
    };

    libraries = lib.mkOption {
      default = {};
      description = ''
        Library deployment targets, keyed by library name (subdirectory under librariesDir).
        Each entry specifies which corpus subdirectory (src) feeds it and how files are
        organized within it (pathSchema). src defaults to "" (entire corpus).
      '';
      example = lib.literalExpression ''
        {
          "music" = {
            pathSchema = with inputs.music-magic.lib.tags; [
              ARTIST ALBUM [ TRACKNUMBER " - " TITLE ]
            ];
          };
          "audiophile" = {
            src = "vinyl-rips";
            pathSchema = with inputs.music-magic.lib.tags; [
              ARTIST ALBUM [ TRACKNUMBER " - " TITLE ]
            ];
          };
        }
      '';
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          src = lib.mkOption {
            type = lib.types.str;
            default = "";
            description = "Corpus subdirectory prefix that feeds this library (relative to srcRoot); empty string means the entire corpus";
          };
          pathSchema = lib.mkOption {
            type = pathSchemaType;
            default = null;
            description = ''
              Path template for organizing deployed files. Either:
              - A list of tag references and string literals (from music-magic.lib.tags)
              - A legacy string like "$ARTIST/$ALBUM/$TRACKNUMBER - $TITLE"
            '';
          };
        };
      });
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

        RuntimeDirectory = baseNameOf cfg.runtimeDir;
        RuntimeDirectoryMode = "0770";

        NoNewPrivileges = true;
        ProtectSystem = "full";
        ProtectHome = true;
        PrivateTmp = true;
      };

      preStart = ''
        mkdir -p ${cfg.dataDir}
        cat > ${cfg.dataDir}/config.kdl <<'EOF'
        storage-root "${cfg.srcRoot}"
        ${lib.optionalString (cfg.librariesDir != null) ''libraries-root "${cfg.librariesDir}"''}
        ${lib.optionalString (cfg.stashDir != null) ''stash-root "${cfg.stashDir}"''}

        ${lib.optionalString cfg.autoDeploy ''
        opinions {
            auto-deploy true
        }
        ''}
        EOF

        ${lib.optionalString (cfg.libraries != {}) ''
        cat > ${cfg.dataDir}/dirs.kdl <<'EOF'
        ${dirsKdl}
        EOF
        ''}

        ${lib.optionalString (cfg.adminPasswordFile != null) ''
          if [ ! -f ${cfg.dataDir}/mm.db ]; then
            ${pkg}/bin/mm --init-user ${cfg.adminUser} --password-file ${cfg.adminPasswordFile}
          fi
        ''}
      '';
    };
  };
}
