//! `nod init` command: scaffold a new Nix flake repository for nod (ADR-021).

use colored::Colorize;
use std::path::PathBuf;

use crate::application::use_cases::scaffold_flake::ScaffoldFlakeUseCase;
use crate::domain::errors::NodError;
use crate::domain::provision::{InitOptions, InitTemplate};

pub fn execute(
    dir: Option<PathBuf>,
    template: InitTemplate,
    name: Option<String>,
) -> Result<(), NodError> {
    let target_dir = dir.unwrap_or_else(|| PathBuf::from("."));
    let flake_name = name.unwrap_or_else(|| {
        target_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "MyFleet".to_string())
    });

    let options = InitOptions {
        target_dir: target_dir.clone(),
        template,
        flake_name,
    };

    let use_case = ScaffoldFlakeUseCase::new();
    use_case.execute(&options)?;

    println!(
        "\n{} Initialized nod repository in '{}'",
        "✓".green().bold(),
        target_dir.display().to_string().bold()
    );
    println!("  • Created {}", "flake.nix".cyan());
    println!("  • Created {}", ".nod.toml".cyan());
    println!("  • Created {}", "hosts/".cyan());
    println!("\nNext steps:");
    println!("  1. Edit {} to define your fleet", "flake.nix".bold());
    println!("  2. Run {} to preview evaluation", "nod plan".bold());
    println!("  3. Run {} to deploy", "nod switch".bold());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_command_creates_valid_scaffold() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test-init");

        let res = execute(Some(path.clone()), InitTemplate::Minimal, None);
        assert!(res.is_ok());
        assert!(path.join("flake.nix").exists());
        assert!(path.join(".nod.toml").exists());
    }
}
