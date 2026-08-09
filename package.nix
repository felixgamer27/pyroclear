{ lib
, rustPlatform
}:

rustPlatform.buildRustPackage {
  pname = "pyroclear";
  version = "0.1.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "Pyroclear";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
