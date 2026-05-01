# manzil — minimalist replacement for Home Manager's `home.files`.
{ config, lib, pkgs, ... }:
let
  inherit (lib)
    attrNames attrValues concatMap concatMapStringsSep filter filterAttrs
    flatten hasPrefix literalExpression mapAttrsToList mkDefault mkEnableOption
    mkIf mkMerge mkOption pipe replaceStrings stringAfter;
  inherit (lib.types)
    anything attrsOf bool functionTo lines nullOr path str submodule;

  osConfig = config; # outer NixOS config — shadowed inside submodules
  cfg = config.manzil;


  # ────────────────────────────────────────────────────────────────────────
  # File entry submodule.
  # `rootDir`         — directory that `target` is resolved against
  # `clobberDefault`  — default for the entry's `clobber` field
  # ────────────────────────────────────────────────────────────────────────
  fileType = { rootDir, clobberDefault }:
    submodule ({ name, config, options, ... }: {
      options = {
        enable = mkEnableOption "this file" // { default = true; };

        relativeTo = mkOption {
          type     = path;
          internal = true;
          default  = rootDir;
        };

        target = mkOption {
          type        = str;
          default     = name;
          defaultText = literalExpression "‹attribute name›";
          apply = p:
            if hasPrefix "/" p
            then throw "manzil: target may not be absolute (got: ${p})"
            else "${config.relativeTo}/${p}";
          description = "Target path, relative to this file set's root.";
        };

        text = mkOption {
          type    = nullOr lines;
          default = null;
          description = "Inline file contents.";
        };

        source = mkOption {
          type    = nullOr path;
          default = null;
          description = "Source file or directory to symlink.";
        };

        executable = mkOption {
          type    = bool;
          default = false;
          description = "Set the +x bit (only with `text` or string-valued `generator`).";
        };

        clobber = mkOption {
          type    = bool;
          default = clobberDefault;
          description = "Overwrite an existing non-symlink target.";
        };

        generator = mkOption {
          type    = nullOr (functionTo anything);
          default = null;
          example = literalExpression "(pkgs.formats.json { }).generate \"x.json\"";
          description = ''
            Function applied to `value`. If the result is a path or derivation
            it becomes the file's `source`; if it is a string it becomes the
            file's `text`. Mutually exclusive with `text`/`source`.
          '';
        };

        value = mkOption {
          type    = nullOr anything;
          default = null;
          description = "Argument passed to `generator`.";
        };
      };

      config = let
        hasGen    = config.generator != null;
        generated = if hasGen then config.generator config.value else null;
        genIsPath = hasGen && options.source.type.check generated;
        genIsText = hasGen && !genIsPath && options.text.type.check generated;
      in mkMerge [
        (mkIf (config.text != null) {
          source = mkDefault (pkgs.writeTextFile {
            name = "manzil-${replaceStrings [ "/" ] [ "-" ] name}";
            inherit (config) text executable;
          });
        })
        (mkIf genIsPath { source = mkDefault generated; })
        (mkIf genIsText { text   = mkDefault generated; })
      ];
    });

  # ────────────────────────────────────────────────────────────────────────
  # Per-user submodule.
  # ────────────────────────────────────────────────────────────────────────
  userType = submodule ({ name, config, ... }: let
    mkFileSet = { rootDir, description }: mkOption {
      type    = attrsOf (fileType {
        inherit rootDir;
        clobberDefault = config.clobberByDefault;
      });
      default = { };
      inherit description;
    };
  in {
    options = {
      enable = mkEnableOption "manzil for this user" // { default = true; };

      directory = mkOption {
        type        = path;
        default     = osConfig.users.users.${name}.home or "/home/${name}";
        defaultText = literalExpression "users.users.<name>.home";
        description = "Home directory; root for `files` targets.";
      };

      clobberByDefault = mkOption {
        type    = bool;
        default = cfg.clobberByDefault;
        description = "Default value of `clobber` for this user's files.";
      };

      files = mkFileSet {
        rootDir     = config.directory;
        description = "Files relative to the user's home directory.";
      };

      xdg.cache.directory = mkOption {
        type        = path;
        default     = "${config.directory}/.cache";
        defaultText = literalExpression ''"$HOME/.cache"'';
        description = "XDG cache root.";
      };
      xdg.cache.files = mkFileSet {
        rootDir     = config.xdg.cache.directory;
        description = "Files relative to the XDG cache directory.";
      };

      xdg.config.directory = mkOption {
        type        = path;
        default     = "${config.directory}/.config";
        defaultText = literalExpression ''"$HOME/.config"'';
        description = "XDG config root.";
      };
      xdg.config.files = mkFileSet {
        rootDir     = config.xdg.config.directory;
        description = "Files relative to the XDG config directory.";
      };

      xdg.data.directory = mkOption {
        type        = path;
        default     = "${config.directory}/.local/share";
        defaultText = literalExpression ''"$HOME/.local/share"'';
        description = "XDG data root.";
      };
      xdg.data.files = mkFileSet {
        rootDir     = config.xdg.data.directory;
        description = "Files relative to the XDG data directory.";
      };

      xdg.state.directory = mkOption {
        type        = path;
        default     = "${config.directory}/.local/state";
        defaultText = literalExpression ''"$HOME/.local/state"'';
        description = "XDG state root.";
      };
      xdg.state.files = mkFileSet {
        rootDir     = config.xdg.state.directory;
        description = "Files relative to the XDG state directory.";
      };
    };
  });

  # All five file sets, flattened to a list of entries.
  allEntries = u: concatMap attrValues [
    u.files
    u.xdg.cache.files
    u.xdg.config.files
    u.xdg.data.files
    u.xdg.state.files
  ];

  mkManifest = name: u:
    let entries = pipe (allEntries u) [
      (filter (f: f.enable && f.source != null))
      (map (f: { inherit (f) target clobber; source = "${f.source}"; }))
    ];
    in pkgs.writeText "manzil-manifest-${name}.json"
      (builtins.toJSON { files = entries; });

  linker       = pkgs.callPackage ./package.nix { };
  enabledUsers = filterAttrs (_: u: u.enable) cfg.users;
  stateDir     = "/var/lib/manzil";

  activationFor = name: userCfg:
    let new = mkManifest name userCfg; in ''
      _old=${stateDir}/manifest-${name}.json
      if ${pkgs.util-linux}/bin/runuser -u ${name} -- \
           ${linker}/bin/manzil ${new} "$_old"; then
        install -m 0644 ${new} "$_old"
      else
        echo "manzil: linker failed for ${name}; state not updated" >&2
      fi
    '';
in
{

  options.manzil = {
    clobberByDefault = mkOption {
      type    = bool;
      default = false;
      description = "Default `clobber` value for every file, every user.";
    };

    users = mkOption {
      type    = attrsOf userType;
      default = { };
      description = "Per-user configurations, keyed by username.";
    };
  };

  config = mkIf (enabledUsers != { }) {
    # Each enabled file must produce a source. `text` and string-valued
    # `generator` populate `source` via mkDefault, so the only failure mode
    # left is "neither set".
    assertions = let
      checkSet = uname: label: files:
        mapAttrsToList (k: f: {
          assertion = !f.enable || f.source != null;
          message =
            "manzil.users.${uname}.${label}.\"${k}\": "
            + "one of `text`, `source`, or `generator` must produce content.";
        }) files;
    in flatten (mapAttrsToList (uname: u: [
      (checkSet uname "files"            u.files)
      (checkSet uname "xdg.cache.files"  u.xdg.cache.files)
      (checkSet uname "xdg.config.files" u.xdg.config.files)
      (checkSet uname "xdg.data.files"   u.xdg.data.files)
      (checkSet uname "xdg.state.files"  u.xdg.state.files)
    ]) enabledUsers);

    system.activationScripts.manzil = stringAfter [ "users" "groups" ] ''
      install -d -m 0755 ${stateDir}
      ${concatMapStringsSep "\n" (n: activationFor n enabledUsers.${n})
        (attrNames enabledUsers)}
    '';

    environment.systemPackages = [ linker ];
  };
}
