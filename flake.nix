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
      };

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
          # The example provider is Python: the protocol is language-agnostic, and
          # its integration test runs the example as a real provider would run.
          python3
        ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      };
    };
}
