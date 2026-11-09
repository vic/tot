# The Rust app is the default package of every system yen builds its own tools on.
{
  yen,
  lib,
  inputs,
  ...
}:
let
  inherit (yen.lib.batteries.lean) systems;
  cargo = (lib.importTOML ../Cargo.toml).package;
  tot =
    pkgs:
    pkgs.rustPlatform.buildRustPackage {
      pname = cargo.name;
      inherit (cargo) version;
      src = lib.fileset.toSource {
        root = ../.;
        fileset = lib.fileset.unions [
          ../Cargo.toml
          ../Cargo.lock
          ../src
          ../data
        ];
      };
      cargoLock.lockFile = ../Cargo.lock;
      # the tests read data/ through CARGO_MANIFEST_DIR and one is a timing test: they run in test.sh
      doCheck = false;
      nativeBuildInputs = [ pkgs.makeWrapper ];
      # The binary defaults to the data and results dirs of its build tree. Those are gone at run time, so the wrapper
      # appends the installed data and a state dir; the first flag the user gives still wins.
      postInstall = ''
        mkdir -p $out/share/tot $out/libexec
        cp -r data $out/share/tot/data
        mv $out/bin/tot $out/libexec/tot
        makeWrapper $out/libexec/tot $out/bin/tot \
          --run 'set -- "$@" --data '$out'/share/tot/data --out "''${XDG_STATE_HOME:-$HOME/.local/state}/tot"'
      '';
      meta.mainProgram = "tot";
    };
in
{
  yen.kinds.fleet.sinks.out.reduce = yen.lib.reduce.merge;
  yen.fleet.main = { };
  yen.flake = { out, ... }: out;

  yen.aspects.tot = {
    at = yen.kinds.fleet;
    out = { ... }: {
      packages = lib.genAttrs systems (
        system:
        let
          pkgs = inputs.nixpkgs.legacyPackages.${system};
        in
        {
          default = tot pkgs;
        }
        # musl, no shared libraries: nixpkgs has no static set on darwin
        // lib.optionalAttrs (lib.hasSuffix "-linux" system) { static-tot = tot pkgs.pkgsStatic; }
      );
    };
  };
}
