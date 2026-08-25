//! `ScaffoldFlakeUseCase`: generate initial repository templates for nod fleets (ADR-021).

use std::fs;

use crate::domain::errors::NodError;
use crate::domain::provision::{InitOptions, InitTemplate};

/// Use case that scaffolds a fresh Nix flake repository with nod configuration.
#[derive(Default)]
pub struct ScaffoldFlakeUseCase;

impl ScaffoldFlakeUseCase {
    pub fn new() -> Self {
        Self
    }

    pub fn execute(&self, options: &InitOptions) -> Result<(), NodError> {
        let dir = &options.target_dir;

        if dir.exists() {
            let entries = fs::read_dir(dir)
                .map_err(|e| NodError::config(format!("failed to read target directory: {e}")))?;
            let has_files = entries.flatten().any(|e| {
                let name = e.file_name();
                name != ".git"
            });
            if has_files {
                return Err(NodError::config(format!(
                    "target directory '{}' is not empty",
                    dir.display()
                )));
            }
        } else {
            fs::create_dir_all(dir)
                .map_err(|e| NodError::config(format!("failed to create directory: {e}")))?;
        }

        let hosts_dir = dir.join("hosts");
        fs::create_dir_all(&hosts_dir)
            .map_err(|e| NodError::config(format!("failed to create hosts directory: {e}")))?;

        // 1. Write flake.nix
        let flake_content = match options.template {
            InitTemplate::Minimal => format!(
                r#"{{
  description = "{} NixOS configuration managed with nod";

  inputs = {{
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nod.url = "github:fleischerdesign/nod";
  }};

  outputs = {{ self, nixpkgs, nod, ... }}: {{
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {{
      system = "x86_64-linux";
      modules = [
        nod.nixosModules.default
        ./hosts/my-host.nix
        {{
          config.nod = {{
            role = "server";
            tags = [ "core" ];
          }};
        }}
      ];
    }};
  }};
}}
"#,
                options.flake_name
            ),
            InitTemplate::Fleet | InitTemplate::Server => format!(
                r#"{{
  description = "{} NixOS Fleet managed with nod";

  inputs = {{
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nod.url = "github:fleischerdesign/nod";
  }};

  outputs = {{ self, nixpkgs, nod, ... }}: {{
    nixosConfigurations = {{
      web-1 = nixpkgs.lib.nixosSystem {{
        system = "x86_64-linux";
        modules = [
          nod.nixosModules.default
          ./hosts/web-1.nix
          {{
            config.nod = {{
              role = "server";
              tags = [ "web", "production" ];
            }};
          }}
        ];
      }};
      db-1 = nixpkgs.lib.nixosSystem {{
        system = "x86_64-linux";
        modules = [
          nod.nixosModules.default
          ./hosts/db-1.nix
          {{
            config.nod = {{
              role = "server";
              tags = [ "db", "production" ];
            }};
          }}
        ];
      }};
    }};
  }};
}}
"#,
                options.flake_name
            ),
        };

        fs::write(dir.join("flake.nix"), flake_content)
            .map_err(|e| NodError::config(format!("failed to write flake.nix: {e}")))?;

        // 2. Write .nod.toml
        let nod_toml = r#"[nod]
flake = "."

[nod.rollout]
strategy = "batch"
batch_size = 2
concurrency = 4
auto_rollback = true

[nod.health]
timeout_secs = 60
"#;
        fs::write(dir.join(".nod.toml"), nod_toml)
            .map_err(|e| NodError::config(format!("failed to write .nod.toml: {e}")))?;

        // 3. Write sample host configs
        match options.template {
            InitTemplate::Minimal => {
                let sample_host = r#"{ config, pkgs, ... }: {
  networking.hostName = "my-host";
  services.openssh.enable = true;
  system.stateVersion = "24.11";
}
"#;
                fs::write(hosts_dir.join("my-host.nix"), sample_host)
                    .map_err(|e| NodError::config(format!("failed to write host config: {e}")))?;
            }
            InitTemplate::Fleet | InitTemplate::Server => {
                let sample_web = r#"{ config, pkgs, ... }: {
  networking.hostName = "web-1";
  services.openssh.enable = true;
  system.stateVersion = "24.11";
}
"#;
                let sample_db = r#"{ config, pkgs, ... }: {
  networking.hostName = "db-1";
  services.openssh.enable = true;
  system.stateVersion = "24.11";
}
"#;
                fs::write(hosts_dir.join("web-1.nix"), sample_web)
                    .map_err(|e| NodError::config(format!("failed to write host config: {e}")))?;
                fs::write(hosts_dir.join("db-1.nix"), sample_db)
                    .map_err(|e| NodError::config(format!("failed to write host config: {e}")))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_creates_files_in_temp_dir() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("my-fleet");

        let opts = InitOptions {
            target_dir: target.clone(),
            template: InitTemplate::Fleet,
            flake_name: "TestFleet".to_string(),
        };

        let use_case = ScaffoldFlakeUseCase::new();
        let res = use_case.execute(&opts);
        assert!(res.is_ok());

        assert!(target.join("flake.nix").exists());
        assert!(target.join(".nod.toml").exists());
        assert!(target.join("hosts/web-1.nix").exists());
    }
}
