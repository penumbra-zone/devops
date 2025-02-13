{
  description = "dev shell for penumbra-operator";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
  # inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          system = "x86_64-linux";
      };
      in
      {
        devShells.default = pkgs.mkShell {
          name = "penumbra-operator devShell";
          nativeBuildInputs = [ pkgs.bashInteractive ];
          buildInputs = with pkgs; [
            doctl
            fd
            file
            fzf
            glibcLocales
            go
            gum
            jq
            just
            k9s
            kubectl
            kubernetes-helm
            minikube
            perl
            rsync
            ruff
            shellcheck
            xz
            yamllint
            yq
          ];
        };
        # Don't automatically source the env, which requires an `age` privkey
        # to load secrets. Might not be available in CI, and we still want to
        # access the nix env via `nix develop`.
        # shellHook = ''
        #   source ./tools/env.sh
        # '';
      });
}
