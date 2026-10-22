//! Rolling a host back to its previous generation without a flake.
//!
//! `nixos-rebuild --rollback switch` looks like the obvious answer and is the
//! wrong one on a flake system: `nixos-rebuild` resolves the default flake at
//! `/etc/nixos` and evaluates `nixosConfigurations.<hostname>` before it
//! considers the rollback. On a host whose flake lives anywhere else - or
//! nowhere, because the closure was copied in - the command fails with
//! `getting the HEAD of the Git tree '/etc/nixos' failed` and the host is left
//! on the generation the rollback was supposed to leave.
//!
//! A rollback needs no flake at all. The generations are on disk: switch to the
//! one before the current, and point the profile at it so a reboot does not
//! undo the rollback.

/// Shell script that activates the previous system generation.
///
/// It resolves the current generation through the `system` profile, walks the
/// `system-*-link` entries in version order, and keeps the one immediately
/// before the current. That handles gaps in generation numbers, which a
/// `N-1` guess does not.
pub const ROLLBACK_SCRIPT: &str = r#"set -eu
current=$(readlink -f /nix/var/nix/profiles/system)
previous=
for link in $(ls -1d /nix/var/nix/profiles/system-*-link 2>/dev/null | sort -V); do
  target=$(readlink -f "$link")
  [ "$target" = "$current" ] && break
  previous=$target
done
if [ -z "$previous" ]; then
  echo "nod: no previous generation to roll back to" >&2
  exit 1
fi
if [ "$(id -u)" -eq 0 ]; then
  nix-env -p /nix/var/nix/profiles/system --set "$previous"
else
  sudo nix-env -p /nix/var/nix/profiles/system --set "$previous"
fi
exec "$previous/bin/switch-to-configuration" switch
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rollback_does_not_consult_a_flake() {
        // The regression this module exists for: `nixos-rebuild` evaluates
        // `/etc/nixos` and fails before the rollback happens.
        assert!(!ROLLBACK_SCRIPT.contains("nixos-rebuild"));
    }

    #[test]
    fn the_rollback_switches_to_a_generation_and_pins_the_profile() {
        assert!(ROLLBACK_SCRIPT.contains("switch-to-configuration"));
        assert!(ROLLBACK_SCRIPT.contains("nix-env -p /nix/var/nix/profiles/system --set"));
        assert!(ROLLBACK_SCRIPT.contains("system-*-link"));
    }

    #[test]
    fn a_missing_previous_generation_is_an_error_not_a_no_op() {
        assert!(ROLLBACK_SCRIPT.contains("no previous generation"));
        assert!(ROLLBACK_SCRIPT.contains("exit 1"));
    }
}
