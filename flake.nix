{
  description = "Universal Nix Orchestration & Deployment Engine (nod)";

  inputs = {
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs-unstable, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs-unstable {
        inherit system;
        config.allowUnfree = true;
      };
    in
    {
      nixosModules.default = import ./modules/nixos/default.nix;
      nixosModules.nod = self.nixosModules.default;
      nixosModules.fleet = import ./modules/nixos/fleet.nix;

      lib.nod = import ./lib/nod.nix { lib = nixpkgs-unstable.lib; };

      packages.${system} = {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "nod";
          version = "3.0.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [
            pkgs.pkg-config
            # The adapter tests spawn git; the build sandbox has no host git.
            pkgs.git
          ];
          buildInputs = [
            pkgs.openssl
          ];
        };

        # Providers are separate packages on purpose: they carry their own
        # dependencies and must not pull an HTTP client into the core.
        nod-provider-cloudflare = pkgs.rustPlatform.buildRustPackage {
          pname = "nod-provider-cloudflare";
          version = "0.1.0";
          src = pkgs.lib.fileset.toSource {
            root = ./providers/cloudflare;
            fileset = pkgs.lib.fileset.unions [
              ./providers/cloudflare/Cargo.toml
              ./providers/cloudflare/Cargo.lock
              ./providers/cloudflare/src
            ];
          };
          cargoLock.lockFile = ./providers/cloudflare/Cargo.lock;
          postInstall = ''
            install -Dm644 ${./providers/cloudflare/provider.json} $out/bin/provider.json
          '';
        };

        nod-provider-authentik = pkgs.rustPlatform.buildRustPackage {
          pname = "nod-provider-authentik";
          version = "0.1.0";
          src = pkgs.lib.fileset.toSource {
            root = ./providers/authentik;
            fileset = pkgs.lib.fileset.unions [
              ./providers/authentik/Cargo.toml
              ./providers/authentik/Cargo.lock
              ./providers/authentik/src
            ];
          };
          cargoLock.lockFile = ./providers/authentik/Cargo.lock;
          postInstall = ''
            install -Dm644 ${./providers/authentik/provider.json} $out/bin/provider.json
          '';
        };

        nod-provider-example = pkgs.writeShellApplication {
          name = "nod-provider-example";
          runtimeInputs = [ pkgs.python3 ];
          text = ''
            exec python3 ${./providers/example/nod-provider-example.py} "$@"
          '';
        };
      };

      # Building the providers is part of the gate: a provider that does not
      # build is not a provider.
      checks.${system}.providers = pkgs.runCommand "nod-providers-check" { } ''
        test -x ${self.packages.${system}.nod-provider-cloudflare}/bin/nod-provider-cloudflare
        test -f ${self.packages.${system}.nod-provider-cloudflare}/bin/provider.json
        test -x ${self.packages.${system}.nod-provider-authentik}/bin/nod-provider-authentik
        test -f ${self.packages.${system}.nod-provider-authentik}/bin/provider.json
        test -x ${self.packages.${system}.nod-provider-example}/bin/nod-provider-example
        touch $out
      '';

      apps.${system}.default = {
        type = "app";
        program = "${self.packages.${system}.default}/bin/nod";
      };

      devShells.${system}.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs; [
          cargo
          rustc
          rust-analyzer
          clippy
          rustfmt
          pkg-config
          openssl
          lix
        ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      };
    };
}
