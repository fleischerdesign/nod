# Nod fleet declaration as a typed NixOS module.
#
# The host profile (`options.nod`, `nixosModules.default`) describes one
# machine; this module describes the fleet a reconciler manages. It is data:
# the flake output `nod = config.services.nod.fleet` is what the daemon reads,
# either as a value (this module) or from a `.nod.toml` file (other users).
#
# The option names are the Nix names (`interval`, `maxDeletions`); `fleet`
# projects them onto the protocol names (`interval_secs`, `max_deletions`) in
# exactly one place. Nothing here knows a provider, a kind or a fleet.
{ config, lib, ... }:
let
  cfg = config.services.nod;

  # One resource in an inline declaration. The provider's constructors type the
  # attributes; the core's contract is kind, id and an attributes object.
  resource = lib.types.submodule {
    options = {
      kind = lib.mkOption {
        type = lib.types.str;
        description = "Resource kind, e.g. `authentik.group`.";
      };
      id = lib.mkOption {
        type = lib.types.str;
        description = "Provider-scoped identifier.";
      };
      attributes = lib.mkOption {
        type = lib.types.attrs;
        default = { };
        description = "Provider-specific attributes; the declaration names a subset.";
      };
    };
  };

  derivation = lib.types.submodule {
    options = {
      id = lib.mkOption {
        type = lib.types.str;
        description = "Stable rule id; used for provenance and the deletion budget.";
      };
      from = lib.mkOption {
        type = lib.types.submodule {
          options = {
            kind = lib.mkOption { type = lib.types.str; };
            id = lib.mkOption { type = lib.types.str; };
            attribute = lib.mkOption { type = lib.types.str; };
          };
        };
      };
      template = lib.mkOption {
        type = lib.types.submodule {
          options = {
            kind = lib.mkOption { type = lib.types.str; };
            id = lib.mkOption { type = lib.types.str; };
            attributes = lib.mkOption {
              type = lib.types.attrs;
              default = { };
            };
          };
        };
      };
      maxDeletions = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 0;
        description = "How many derived removals this rule may cause in one run.";
      };
    };
  };

  document = lib.types.submodule {
    options = {
      schema = lib.mkOption {
        type = lib.types.ints.between 1 2;
        default = 2;
        description = "Desired-state schema; `derive` requires 2.";
      };
      resources = lib.mkOption {
        type = lib.types.listOf resource;
        default = [ ];
      };
      absent = lib.mkOption {
        type = lib.types.listOf resource;
        default = [ ];
      };
      derive = lib.mkOption {
        type = lib.types.listOf derivation;
        default = [ ];
      };
    };
  };

  provider = lib.types.submodule {
    options = {
      command = lib.mkOption {
        type = lib.types.str;
        description = "Provider executable.";
      };
      settings = lib.mkOption {
        type = lib.types.attrs;
        default = { };
        description = "Provider settings; validated by the provider's handshake.";
      };
      desired = lib.mkOption {
        type = lib.types.oneOf [
          lib.types.str
          lib.types.path
          document
        ];
        description = ''
          The declaration: an inline document, or a path to a JSON file. A flake
          carries the document; a configuration file names a file.
        '';
      };
      apply = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Whether the daemon may apply changes (default: dry-run only).";
      };
      interval = lib.mkOption {
        type = lib.types.ints.positive;
        default = 300;
        description = "Cadence in seconds for the daemon.";
      };
      timeout = lib.mkOption {
        type = lib.types.ints.positive;
        default = 60;
        description = "Timeout for one provider operation in seconds.";
      };
      order = lib.mkOption {
        type = lib.types.enum [
          "before-hosts"
          "after-hosts"
          "independent"
        ];
        default = "independent";
        description = "Where the provider runs relative to the host sweep.";
      };
      env = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = "Environment variable names the provider inherits.";
      };
      policy = lib.mkOption {
        type = lib.types.submodule {
          options = {
            protect = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              description = "Resource id patterns that may never be deleted.";
            };
            maxDeletions = lib.mkOption {
              type = lib.types.ints.unsigned;
              default = 0;
              description = "Maximum implicit deletions in one run.";
            };
          };
        };
        default = { };
        description = "What the core allows this provider's runs to delete.";
      };
    };
  };

  # A document is an attrset with `resources` (the submodule coerces it); a path
  # or string is passed through untouched.
  projectDesired =
    desired:
    if builtins.isAttrs desired && (desired ? resources) then desired else toString desired;
in
{
  options.services.nod = {
    schema = lib.mkOption {
      type = lib.types.ints.between 1 2;
      default = 2;
      description = "Schema version of the fleet output.";
    };

    providers = lib.mkOption {
      type = lib.types.attrsOf provider;
      default = { };
      description = "Providers the reconciler manages, by name.";
    };

    fleet = lib.mkOption {
      type = lib.types.attrs;
      readOnly = true;
      description = ''
        The fleet declaration in the shape the daemon reads. The flake output is
        `nod = config.services.nod.fleet`; the Nix option names are mapped to the
        protocol names here, and nowhere else.
      '';
      default = {
        inherit (cfg) schema;
        providers = lib.mapAttrs (_: entry: {
          inherit (entry) command settings apply env;
          desired = projectDesired entry.desired;
          interval_secs = entry.interval;
          timeout_secs = entry.timeout;
          inherit (entry) order;
          policy = {
            inherit (entry.policy) protect;
            max_deletions = entry.policy.maxDeletions;
          };
        }) cfg.providers;
      };
    };
  };
}
