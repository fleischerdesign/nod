# Nod NixOS module: the complete `nod` configuration surface.
#
# This module declares every option that the nod orchestration engine reads
# from a machine's `config.nod` (ADR-004 tier 3). The Rust domain structs under
# `src/domain/config.rs` mirror this surface 1:1 in camelCase / snake_case so a
# value set here round-trips through Nix-evaluated JSON (camelCase) and
# `.nod.toml` (snake_case) unchanged.
#
# The whole `options.nod` tree is what the `NixCliEvaluator` emits as the
# `nod = x.nod` field during host discovery, so the granular ssh / build /
# rollout / healthChecks / hooks values deserialize directly onto the
# `HostEntity.nod_config`.

{ lib, config, ... }:
{

  options.nod = {

    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Whether the Nod agent is enabled for this machine. When disabled, the
        host is treated as a plain flake target with no orchestration profile.
      '';
    };

    targetHost = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = config.networking.hostName;
      description = ''
        The hostname or address nod uses to reach this machine. Falls back to
        `networking.hostName` when unset.
      '';
    };

    role = lib.mkOption {
      type = lib.types.enum [
        "server"
        "desktop"
        "notebook"
        "router"
        "embedded"
        "custom"
      ];
      default = "server";
      description = ''
        The functional role of the host within a fleet.
      '';
    };

    tags = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = ''
        Operator-assignable tags used for fleet filtering (`--tag`).
      '';
    };

    description = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        Optional human-readable description of the host.
      '';
    };

    ssh = lib.mkOption {
      type = lib.types.submodule {
        options = {
          user = lib.mkOption {
            type = lib.types.str;
            default = "root";
            description = ''
              SSH user used to reach this host.
            '';
          };
          port = lib.mkOption {
            type = lib.types.port;
            default = 22;
            description = ''
              SSH port used to reach this host.
            '';
          };
          identityFile = lib.mkOption {
            type = lib.types.nullOr lib.types.path;
            default = null;
            description = ''
              Path to the SSH private key used for authentication.
            '';
          };
          proxyJump = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = ''
              Intermediate host (or `user@host:port`) to proxy through.
            '';
          };
          proxyCommand = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = ''
              Explicit SSH command used to reach the target (e.g.
              `ssh -W %d:%p bastion`).
            '';
          };
          sudo = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              Whether the switch command should be escalated with sudo.
            '';
          };
          timeoutSecs = lib.mkOption {
            type = lib.types.ints.positive;
            default = 30;
            description = ''
              Overall SSH session timeout in seconds.
            '';
          };
          connectTimeoutSecs = lib.mkOption {
            type = lib.types.ints.positive;
            default = 10;
            description = ''
              SSH connection-establishment timeout in seconds.
            '';
          };
          extraSshArgs = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
            description = ''
              Additional raw arguments appended to the SSH command line.
            '';
          };
          allowInsecure = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              Allow insecure-but-needed SSH session options.
            '';
          };
        };
      };
      default = {
        user = "root";
        port = 22;
        sudo = false;
        timeoutSecs = 30;
        connectTimeoutSecs = 10;
        extraSshArgs = [];
        allowInsecure = false;
      };
      description = ''
        SSH connection profile for this host (1:1 with `SshProfileConfig` /
        the `[hosts.<name>]` TOML SSH keys).
      '';
    };

    build = lib.mkOption {
      type = lib.types.submodule {
        options = {
          buildHost = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = ''
              Machine that builds the toplevel closure for this host.
            '';
          };
          substituters = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
            description = ''
              Extra substituter URIs used during evaluation.
            '';
          };
          trustedPublicKeys = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
            description = ''
              Public keys trusted for closure signing.
            '';
          };
          evalFlags = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
            description = ''
              Extra `nix eval` flags applied when evaluating this host.
            '';
          };
          showTrace = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              Emit evaluation trace output for this host.
            '';
          };
          impure = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              Allow the use of non-pure values during evaluation.
            '';
          };
        };
      };
      default = {
        substituters = [];
        trustedPublicKeys = [];
        evalFlags = [];
        showTrace = false;
        impure = false;
      };
      description = ''
        Build customisation options (1:1 with `options.nod.build`).
      '';
    };

    rollout = lib.mkOption {
      type = lib.types.submodule {
        options = {
          priority = lib.mkOption {
            type = lib.types.ints.unsigned;
            default = 100;
            description = ''
              Lower numbers take precedence during a fleet rollout.
            '';
          };
          action = lib.mkOption {
            type = lib.types.enum [
              "switch"
              "boot"
              "test"
              "dry-run"
            ];
            default = "switch";
            description = ''
              The activation action performed by the switch command.
            '';
          };
          autoRollback = lib.mkOption {
            type = lib.types.bool;
            default = true;
            description = ''
              Automatically roll back on a failed activation.
            '';
          };
          reboot = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              Reboot the host after activation.
            '';
          };
          magicRollback = lib.mkOption {
            type = lib.types.bool;
            default = true;
            description = ''
              Use the magic quiescence-based rollback scheme.
            '';
          };
          magicRollbackTimeoutSecs = lib.mkOption {
            type = lib.types.ints.positive;
            default = 60;
            description = ''
              How long the magic rollback waits for quiescence.
            '';
          };
        };
      };
      default = {
        priority = 100;
        action = "switch";
        autoRollback = true;
        reboot = false;
        magicRollback = true;
        magicRollbackTimeoutSecs = 60;
      };
      description = ''
        Rollout / activation options (1:1 with `options.nod.rollout`).
      '';
    };

    healthChecks = lib.mkOption {
      type = lib.types.submodule {
        options = {
          enable = lib.mkOption {
            type = lib.types.bool;
            default = true;
            description = ''
              Whether health checks run after activation.
            '';
          };
          timeoutSecs = lib.mkOption {
            type = lib.types.ints.positive;
            default = 30;
            description = ''
              Global health-check timeout in seconds.
            '';
          };
          systemd = lib.mkOption {
            type = lib.types.submodule {
              options = {
                checkRunning = lib.mkOption {
                  type = lib.types.bool;
                  default = true;
                  description = ''
                    Verify that required systemd units are running.
                  '';
                };
                checkFailedUnits = lib.mkOption {
                  type = lib.types.bool;
                  default = true;
                  description = ''
                    Report any systemd units in a failed state.
                  '';
                };
                requiredUnits = lib.mkOption {
                  type = lib.types.listOf lib.types.str;
                  default = [];
                  description = ''
                    systemd units that must be active after activation.
                  '';
                };
              };
            };
            default = {
              checkRunning = true;
              checkFailedUnits = true;
              requiredUnits = [];
            };
            description = ''
              systemd unit verification sub-settings.
            '';
          };
          tcpPorts = lib.mkOption {
            type = lib.types.listOf lib.types.port;
            default = [];
            description = ''
              TCP ports that must be listening after activation.
            '';
          };
          httpProbes = lib.mkOption {
            type = lib.types.listOf (
              lib.types.submodule {
                options = {
                  url = lib.mkOption {
                    type = lib.types.str;
                    description = ''
                      URL to probe over HTTP(S).
                    '';
                  };
                  expectedStatus = lib.mkOption {
                    type = lib.types.int;
                    default = 200;
                    description = ''
                      The HTTP status code expected from the probe.
                    '';
                  };
                  timeoutSecs = lib.mkOption {
                    type = lib.types.ints.positive;
                    default = 10;
                    description = ''
                      Per-probe timeout in seconds.
                    '';
                  };
                };
              }
            );
            default = [];
            description = ''
              HTTP probes that must succeed after activation.
            '';
          };
          customProbes = lib.mkOption {
            type = lib.types.listOf (
              lib.types.submodule {
                options = {
                  name = lib.mkOption {
                    type = lib.types.str;
                    description = ''
                      Stable name for this custom probe.
                    '';
                  };
                  command = lib.mkOption {
                    type = lib.types.str;
                    description = ''
                      Shell command that checks the host; a zero exit status
                      indicates a passing probe.
                    '';
                  };
                  timeoutSecs = lib.mkOption {
                    type = lib.types.ints.positive;
                    default = 10;
                    description = ''
                      Per-probe timeout in seconds.
                    '';
                  };
                };
              }
            );
            default = [];
            description = ''
              Custom command-based probes that must succeed after activation.
            '';
          };
        };
      };
      default = {
        enable = true;
        timeoutSecs = 30;
        tcpPorts = [];
        httpProbes = [];
        customProbes = [];
      };
      description = ''
        Health-verification options (1:1 with `options.nod.healthChecks`).
      '';
    };

    hooks = lib.mkOption {
      type = lib.types.submodule {
        options = {
          preSwitchHook = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = ''
              Command run immediately before activation.
            '';
          };
          postSwitchHook = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = ''
              Command run immediately after activation.
            '';
          };
        };
      };
      default = {
        preSwitchHook = null;
        postSwitchHook = null;
      };
      description = ''
        Switch lifecycle hooks (1:1 with `options.nod.hooks`).
      '';
    };

  };
}