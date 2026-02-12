{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.cosmic-ext-rdp-server;
  settingsFormat = pkgs.formats.toml { };

  # Build the TOML config, merging auth settings only when enabled.
  effectiveSettings = cfg.settings // optionalAttrs cfg.auth.enable {
    auth = {
      enable = true;
      username = cfg.auth.username;
      # Password is read from passwordFile at service start via
      # LoadCredential, then injected with a wrapper script.
    } // optionalAttrs (cfg.auth.domain != null) {
      domain = cfg.auth.domain;
    };
  };

  configFile = settingsFormat.generate "cosmic-ext-rdp-server.toml" effectiveSettings;

  # Wrapper script that injects the password from the credential file
  # into the TOML config at runtime, then execs the server.
  # Uses only shell builtins (no sed/awk) to avoid SIGSYS from SystemCallFilter.
  startScript = pkgs.writeShellScript "cosmic-ext-rdp-server-start" ''
    CONFIG="${configFile}"

    if [ -n "''${CREDENTIALS_DIRECTORY:-}" ] && [ -f "$CREDENTIALS_DIRECTORY/rdp-password" ]; then
      RUNTIME_CONFIG="''${RUNTIME_DIRECTORY}/config.toml"
      PASSWORD=$(<"$CREDENTIALS_DIRECTORY/rdp-password")
      # Escape backslashes and double quotes for safe TOML string embedding
      PASSWORD="''${PASSWORD//\\/\\\\}"
      PASSWORD="''${PASSWORD//\"/\\\"}"
      # Rebuild config with password injected after [auth] using only shell builtins.
      # Use umask (builtin) to ensure restrictive permissions on the output file.
      umask 0177
      while IFS= read -r line; do
        printf '%s\n' "$line"
        if [ "$line" = "[auth]" ]; then
          printf 'password = "%s"\n' "$PASSWORD"
        fi
      done < "$CONFIG" > "$RUNTIME_CONFIG"
      CONFIG="$RUNTIME_CONFIG"
    fi

    exec ${cfg.package}/bin/cosmic-ext-rdp-server --config "$CONFIG"
  '';
in
{
  options.services.cosmic-ext-rdp-server = {
    enable = mkEnableOption "RDP Server for COSMIC for remote desktop access";

    package = mkPackageOption pkgs "cosmic-ext-rdp-server" {
      default = [ "cosmic-ext-rdp-server" ];
      example = literalExpression ''
        pkgs.cosmic-ext-rdp-server.override { }
      '';
    };

    installSettings = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to install the RDP Settings for COSMIC GUI application.";
    };

    settingsPackage = mkPackageOption pkgs "cosmic-ext-rdp-settings" {
      default = [ "cosmic-ext-rdp-settings" ];
      example = literalExpression ''
        pkgs.cosmic-ext-rdp-settings
      '';
    };

    auth = {
      enable = mkEnableOption "NLA (Network Level Authentication) via CredSSP";

      username = mkOption {
        type = types.str;
        default = "";
        description = "Username for NLA authentication.";
      };

      domain = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Windows domain for NLA authentication (optional).";
      };

      passwordFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to a file containing the NLA password.

          The file should contain only the password (no trailing newline).
          This is loaded via systemd LoadCredential so it never appears
          in the Nix store. Compatible with agenix/sops-nix secrets.
        '';
        example = "/run/agenix/cosmic-ext-rdp-password";
      };
    };

    settings = mkOption {
      type = types.submodule {
        freeformType = settingsFormat.type;
        options = {
          bind = mkOption {
            type = types.str;
            default = "127.0.0.1:3389";
            description = "Address and port to bind the RDP server to.";
          };

          capture = mkOption {
            type = types.submodule {
              freeformType = settingsFormat.type;
              options = {
                fps = mkOption {
                  type = types.int;
                  default = 30;
                  description = "Target frames per second for screen capture.";
                };
                channel_capacity = mkOption {
                  type = types.int;
                  default = 4;
                  description = "PipeWire channel capacity (number of buffered frames).";
                };
                multi_monitor = mkOption {
                  type = types.bool;
                  default = false;
                  description = "Enable multi-monitor capture (merges all selected monitors into a single virtual desktop).";
                };
              };
            };
            default = { };
            description = "Screen capture settings.";
          };

          audio = mkOption {
            type = types.submodule {
              freeformType = settingsFormat.type;
              options = {
                enable = mkOption {
                  type = types.bool;
                  default = true;
                  description = "Enable RDPSND audio forwarding from the desktop to the RDP client.";
                };
                sample_rate = mkOption {
                  type = types.int;
                  default = 44100;
                  description = "Audio sample rate in Hz.";
                };
                channels = mkOption {
                  type = types.int;
                  default = 2;
                  description = "Number of audio channels (1 = mono, 2 = stereo).";
                };
              };
            };
            default = { };
            description = "Audio forwarding settings (RDPSND).";
          };

          encode = mkOption {
            type = types.submodule {
              freeformType = settingsFormat.type;
              options = {
                encoder = mkOption {
                  type = types.enum [ "auto" "vaapi" "nvenc" "software" ];
                  default = "auto";
                  description = "Preferred video encoder backend.";
                };
                preset = mkOption {
                  type = types.str;
                  default = "ultrafast";
                  description = "H.264 encoding preset.";
                };
                bitrate = mkOption {
                  type = types.int;
                  default = 10000000;
                  description = "Target bitrate in bits per second.";
                };
              };
            };
            default = { };
            description = "Video encoding settings.";
          };
        };
      };
      default = { };
      description = ''
        Configuration for the RDP Server for COSMIC.

        Settings are written to a TOML configuration file.
        See the project README for all available options.
      '';
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Whether to open the RDP port in the firewall.";
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = mkIf cfg.installSettings [ cfg.settingsPackage ];

    assertions = [
      {
        assertion = cfg.auth.enable -> cfg.auth.username != "";
        message = "services.cosmic-ext-rdp-server.auth.username must be set when auth is enabled.";
      }
      {
        assertion = cfg.auth.enable -> cfg.auth.passwordFile != null;
        message = "services.cosmic-ext-rdp-server.auth.passwordFile must be set when auth is enabled.";
      }
    ];

    systemd.user.services.cosmic-ext-rdp-server = {
      description = "RDP Server for COSMIC";
      after = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];

      serviceConfig = {
        Type = "simple";
        ExecStart = toString startScript;
        Restart = "on-failure";
        RestartSec = 5;
        RuntimeDirectory = "cosmic-ext-rdp-server";

        # Load password from file without storing in Nix store
        LoadCredential = optional (cfg.auth.enable && cfg.auth.passwordFile != null)
          "rdp-password:${cfg.auth.passwordFile}";

        # Security hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        RestrictSUIDSGID = true;
        RestrictNamespaces = true;
        LockPersonality = true;
        # Note: MemoryDenyWriteExecute, RestrictRealtime, and SystemCallFilter
        # are intentionally omitted. PipeWire's RT module needs realtime
        # scheduling (sched_setscheduler), and the sspi/CredSSP library needs
        # syscalls from @privileged/@resources groups. These are incompatible
        # with strict syscall filtering. The remaining hardening options
        # (NoNewPrivileges, ProtectSystem=strict, etc.) provide adequate
        # protection for a user service.
      };
    };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [
      (
        let
          parts = splitString ":" cfg.settings.bind;
          port = last parts;
        in
        toInt port
      )
    ];
  };
}
