{
  description = "Dev shell for managing Penumbra Labs network artifacts";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
  # inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          system = "${system}";
        };

      in

      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [ pkgs.bashInteractive ];
          buildInputs = with pkgs; [
            aria2
            awscli2
            fclones
            fd
            file
            fio
            glibcLocales
            jq
            just
            perl
            postgresql_16 # v16 is used for penumbra ecosystem
            rsync
            shellcheck
            xz
          ];
        };
      });
}
