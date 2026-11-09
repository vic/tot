# The dev shell of each system: the Rust toolchain, and vhs to record the demo (`vhs demo/cyberpunk.tape`).
# `tot` in the shell is the flake's own default package.
{
  yen,
  inputs,
  ...
}:
let
  inherit (yen.lib.batteries) devshell;
in
{
  yen.kinds.fleet.spawns.system = _: yen.system;
  yen.system.x86_64-linux = { };
  yen.system.aarch64-linux = { };
  yen.system.aarch64-darwin = { };

  yen.kinds.system.sinks.shell.out = devshell.mkshell {
    from = yen.kinds.system.sinks.shell;
    pkgsOf = s: inputs.nixpkgs.legacyPackages.${s.name};
  };

  yen.aspects.devshell = {
    at = yen.kinds.system;
    shell =
      { system, out, ... }:
      let
        pkgs = inputs.nixpkgs.legacyPackages.${system.name};
      in
      {
        packages = [
          pkgs.cargo
          pkgs.rustc
          pkgs.clippy
          pkgs.vhs
          out.packages.${system.name}.default
        ];
      };
  };
}
