{
  description = "Dev shell for pindexer-db-wrangler tool";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
  # inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  inputs.rust-overlay.url = "github:oxalica/rust-overlay";
  inputs.rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

  # We declare two different versions of upstream `pindexer`
  # deps, to enable handling different network environments differently.
  inputs.penumbra-repo-mainnet = {
    url = "github:penumbra-zone/penumbra/v1.3.1";
    # Reuse the nixpkgs from the current devshell.
    inputs.nixpkgs.follows = "nixpkgs"; # Use your nixpkgs
  };

  inputs.penumbra-repo-testnet = {
    url = "github:penumbra-zone/penumbra/v2.0.0-alpha.4";
    # Reuse the nixpkgs from the current devshell.
    inputs.nixpkgs.follows = "nixpkgs"; # Use your nixpkgs
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, penumbra-repo-mainnet, penumbra-repo-testnet }:

    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          system = "${system}";
          overlays = [ rust-overlay.overlays.default ];
        };

        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in

      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          version = cargoToml.package.version;
          src = ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
        };

        devShells.default = pkgs.mkShell {
          name = "devShell";
          nativeBuildInputs = [ pkgs.bashInteractive ];
          buildInputs = with pkgs; [
            # Wrap the `pindexer` command in a script to avoid a `cannot execute binary` error.
            (pkgs.writeShellScriptBin "pindexer-mainnet" ''
              exec ${penumbra-repo-mainnet.apps.${system}.pindexer.program} "$@"
            '')
            (pkgs.writeShellScriptBin "pindexer-testnet" ''
              exec ${penumbra-repo-testnet.apps.${system}.pindexer.program} "$@"
            '')
            rust-bin.stable.latest.default
            fd
            file
            fio
            glibcLocales
            go
            jq
            just
            kubectl
            openssl
            perl
            postgresql_16 # v16 is used for penumbra ecosystem
            process-compose
            rsync
            ruff
            shellcheck
            xz
            yamllint
            yq-go
          ];

          # We override the PYTHONPATH setting so that Ansible can import the psycopg2 module.
          shellHook = ''
          '';
        };
      });
}
