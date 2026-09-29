{
  description = "manzil — minimalist home files";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {nixpkgs, ...}: {
    nixosModules.default = ./nix/modules/nixos.nix;
    nixosModules.manzil = ./nix/modules/nixos.nix;

    darwinModules.default = ./nix/modules/darwin.nix;
    darwinModules.manzil = ./nix/modules/darwin.nix;

    finixModules.default = ./nix/modules/finix.nix;
    finixModules.manzil = ./nix/modules/finix.nix;

    packages =
      nixpkgs.lib.recursiveUpdate ((nixpkgs.lib.genAttrs ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"]) (system: rec {
        manzil = nixpkgs.legacyPackages."${system}".callPackage ./nix/package.nix {};
        default = manzil;
      })) {
        x86_64-linux.manzil-aarch64-linux-static = (nixpkgs.legacyPackages.x86_64-linux.pkgsCross.aarch64-multiplatform-musl.callPackage ./nix/package.nix {}).overrideAttrs (old: {
          env = old.env // {RUSTFLAGS = "-C target-feature=+crt-static";};
        });
      };

    checks = (nixpkgs.lib.genAttrs ["x86_64-linux" "aarch64-linux"]) (system: {
      manzil-module = import ./tests/module.nix {inherit nixpkgs system;};
    });
  };
}
