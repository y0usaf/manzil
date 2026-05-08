{
  description = "manzil — minimalist home files";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {nixpkgs, ...}: {
    nixosModules.default = ./nix/modules/nixos.nix;
    nixosModules.manzil = ./nix/modules/nixos.nix;

    darwinModules.default = ./nix/modules/darwin.nix;
    darwinModules.manzil = ./nix/modules/darwin.nix;

    packages = (nixpkgs.lib.genAttrs ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"]) (system: rec {
      manzil = nixpkgs.legacyPackages."${system}".callPackage ./nix/package.nix {};
      default = manzil;
    });
  };
}
