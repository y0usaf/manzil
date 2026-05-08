{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "manzil";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      (lib.fileset.maybeMissing ../tests)
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  meta = {
    description = "Minimalist home file reconciler";
    mainProgram = "manzil";
    license = lib.licenses.mit;
    platforms = lib.platforms.unix;
  };
}
