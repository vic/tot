# tot as wasm32-wasip1 for the browser (worker.js runs it on a WASI shim).
let
  pkgs = (builtins.getFlake "nixpkgs").legacyPackages.x86_64-linux.pkgsCross.wasi32;
  root = ../.;
in
pkgs.rustPlatform.buildRustPackage {
  pname = "tot";
  version = "0.1.0";
  src = pkgs.lib.fileset.toSource {
    inherit root;
    fileset = pkgs.lib.fileset.unions [ ../Cargo.toml ../Cargo.lock ../src ../data ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  doCheck = false;
  nativeBuildInputs = [ pkgs.buildPackages.lld ];
  # the cross hook points cargo at the clang wrapper, which does not take the wasm-ld flavor rustc passes
  preBuild = "export RUSTFLAGS=\"$RUSTFLAGS -C linker=wasm-ld\"";
}
