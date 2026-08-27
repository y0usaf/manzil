# Module-level tests for the `parser` option: eval a NixOS config per case,
# check the generated file contents, and check that invalid `parser`
# combinations fail evaluation.
{
  nixpkgs,
  system,
}: let
  lib = nixpkgs.lib;
  pkgs = nixpkgs.legacyPackages.${system};
  json = pkgs.formats.json {};

  eval = extra:
    lib.nixosSystem {
      inherit system;
      modules = [
        ../nix/modules/nixos.nix
        {users.users.alice.isNormalUser = true;}
        {manzil.users.alice = extra;}
      ];
    };

  gen = name: entry:
    (eval {xdg.config.files."${name}" = entry;})
    .config
    .manzil
    .users
    .alice
    .xdg
    .config
    .files
    ."${name}";

  # eval must fail; returns the case name on unexpected success, "" otherwise
  mustFail = name: entry: let
    attempt = builtins.tryEval (builtins.seq (gen name entry).source true);
  in
    if attempt.success
    then name
    else "";

  merged = gen "merged.json" {
    generator = json.generate "merged.json";
    parser = "toml";
    base = ./data/base.toml;
    value = {
      theme = "dark";
      extra = true;
      keys.list = ["c"];
    };
  };

  mergedFn = gen "merged-fn.json" {
    generator = json.generate "merged-fn.json";
    parser = path: builtins.fromTOML (builtins.readFile path);
    base = ./data/base.toml;
    value = {theme = "night";};
  };

  passthrough = gen "passthrough.json" {
    generator = json.generate "passthrough.json";
    parser = "json";
    base = ./data/base.json;
  };

  failures =
    lib.concatStringsSep ","
    (lib.filter (s: s != "") [
      (mustFail "no-generator.json" {
        parser = "toml";
        base = ./data/base.toml;
        value = {theme = "dark";};
      })
      (mustFail "with-merge.json" {
        type = "merge";
        format = "toml";
        parser = "toml";
        base = ./data/base.toml;
        value = {theme = "dark";};
      })
      (mustFail "no-base.json" {
        generator = json.generate "no-base.json";
        parser = "toml";
        value = {theme = "dark";};
      })
    ]);
in
  pkgs.runCommand "manzil-module-tests" {
    inherit failures;
    mergedOut = merged.source;
    mergedFnOut = mergedFn.source;
    passthroughOut = passthrough.source;
    nativeBuildInputs = [pkgs.jq];
  } ''
    set -euo pipefail
    if [ -n "$failures" ]; then
      echo "manzil module tests: invalid parser configs evaluated successfully: $failures"
      exit 1
    fi
    # named parser: base merged under value; Nix wins; arrays replaced
    jq -e '.theme == "dark" and .baseOnly == "keep"
           and .keys.quit == "q" and .keys.list == ["c"] and .extra == true' "$mergedOut" >/dev/null
    # function parser: same merge path
    jq -e '.theme == "night" and .keys.quit == "q" and .baseOnly == "keep"' "$mergedFnOut" >/dev/null
    # value = null: parsed base passes through
    jq -e '.alpha == 1 and .nested.x == true' "$passthroughOut" >/dev/null
    touch "$out"
  ''
