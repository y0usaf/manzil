{platform}: {
  config,
  lib,
  pkgs,
  options ? {},
  utils ? null,
  ...
}: let
  inherit (builtins) elem isList toJSON;
  inherit
    (lib)
    any
    attrNames
    attrValues
    concatMap
    concatMapStringsSep
    concatStringsSep
    escapeShellArg
    filter
    filterAttrs
    flatten
    hasPrefix
    length
    literalExpression
    listToAttrs
    mapAttrs
    mapAttrsToList
    mkDefault
    mkEnableOption
    mkIf
    mkMerge
    mkOption
    nameValuePair
    optional
    optionalAttrs
    optionalString
    pipe
    replaceStrings
    splitString
    stringAfter
    unique
    ;
  inherit (lib.meta) getExe;
  inherit
    (lib.types)
    addCheck
    anything
    attrs
    attrsOf
    bool
    deferredModule
    enum
    functionTo
    int
    lines
    listOf
    nullOr
    oneOf
    package
    path
    str
    strMatching
    submodule
    submoduleWith
    ;

  cfg = config.manzil;
  osUsers = config.users.users or {};

  isNixOS = platform == "nixos";
  isDarwin = platform == "darwin";
  isFinix = platform == "finix";
  stateDir =
    if isDarwin
    then "/var/db/manzil"
    else "/var/lib/manzil";
  unitTypes = ["path" "service" "slice" "socket" "target" "timer"];

  nonnegativeInt = addCheck int (n: n >= 0);
  listOrValue = oneOf [str (listOf str)];

  mimeSection = title: attrs:
    optionalString (attrs != {}) (
      "[${title}]\n"
      + concatStringsSep "\n" (mapAttrsToList (mime: apps: "${mime}=${concatStringsSep ";" ((v:
    if isList v
    then v
    else [v]) apps)};") attrs)
      + "\n"
    );

  fileSets = u: [
    {
      label = "files";
      inherit (u) files;
    }
    {
      label = "xdg.cache.files";
      inherit (u.xdg.cache) files;
    }
    {
      label = "xdg.config.files";
      inherit (u.xdg.config) files;
    }
    {
      label = "xdg.data.files";
      inherit (u.xdg.data) files;
    }
    {
      label = "xdg.state.files";
      inherit (u.xdg.state) files;
    }
  ];

  allEntries = u: concatMap (set: attrValues set.files) (fileSets u);

  enabledUsers = filterAttrs (_: u: u.enable) cfg.users;

  activationScript = ''
    ${pkgs.coreutils}/bin/install -d -m 0755 ${escapeShellArg stateDir}
    ${concatMapStringsSep "\n" (n: (name: u: let
    new = (name: u:
    pkgs.writeText "manzil-manifest-${name}.json" (toJSON {
      version = 3;
      files = pipe (allEntries u) [
        (filter (f: f.enable))
        (map (f:
    (filterAttrs (_: v: v != null) {
      inherit (f) type target clobber force permissions uid gid;
      source =
        if f.source == null
        then null
        else "${f.source}";
    })
    // optionalAttrs (f.type == "merge") ({inherit (f) format;}
      // optionalAttrs (f.arrayDefault != "replace") {inherit (f) arrayDefault;}
      // optionalAttrs (f.arrays != {}) {inherit (f) arrays;})))
      ];
    })) name u;
    old = "${stateDir}/manifest-${name}.json";
    home = osUsers."${name}".home or (if isDarwin then "/Users/${name}" else "/home/${name}");
    cmd = "${concatStringsSep " " (map escapeShellArg ([(getExe cfg.linker)] ++ cfg.linkerArgs))} ${escapeShellArg "${new}"} ${escapeShellArg old}";
    body = ''
      if ${if isNixOS
      then "${pkgs.util-linux}/bin/runuser -u ${escapeShellArg name} -- env HOME=${escapeShellArg home} ${cmd}"
      # finix PAM has no runuser service; setpriv does the same uid/gid drop
      # with zero PAM. uid/gid resolved at run time (after the users script).
      else if isFinix
      then "${pkgs.util-linux}/bin/setpriv --reuid \"$(${pkgs.coreutils}/bin/id -u ${escapeShellArg name})\" --regid \"$(${pkgs.coreutils}/bin/id -g ${escapeShellArg name})\" --clear-groups ${pkgs.coreutils}/bin/env HOME=${escapeShellArg home} ${cmd}"
      else "/usr/bin/su -l ${escapeShellArg name} -c ${escapeShellArg cmd}"}; then
        ${pkgs.coreutils}/bin/install -m 0644 ${escapeShellArg "${new}"} ${escapeShellArg old}
      else
        echo ${escapeShellArg "manzil: linker failed for ${name}; state not updated"} >&2
      fi
    '';
  in
    if isDarwin
    then ''
      if /usr/bin/id -u ${escapeShellArg name} >/dev/null 2>&1; then
        _lock=${escapeShellArg "${stateDir}/lock-${name}"}
        (
          while ! ${pkgs.coreutils}/bin/mkdir "$_lock" 2>/dev/null; do sleep 1; done
          trap '${pkgs.coreutils}/bin/rmdir "$_lock"' EXIT
          ${body}
        )
      else
        echo ${escapeShellArg "manzil: user ${name} does not exist; skipped"} >&2
      fi
    ''
    else ''
      _lock=${escapeShellArg "${stateDir}/lock-${name}"}
      (
        ${pkgs.util-linux}/bin/flock -x 9
        ${body}
      ) 9>"$_lock"
    '') n enabledUsers."${n}") (attrNames enabledUsers)}
  '';
in {
  options.manzil = {
    clobberByDefault = mkOption {
      type = bool;
      default = false;
      description = "Default `clobber` value for every file, every user.";
    };

    linker = mkOption {
      type = package;
      default = pkgs.callPackage ../package.nix {};
      defaultText = literalExpression "pkgs.callPackage ./nix/package.nix { }";
      description = "Linker backend package; executable must accept `new-manifest [old-manifest]`.";
    };

    linkerArgs = mkOption {
      type = listOf str;
      default = [];
      description = "Extra arguments passed to the linker before manifest paths.";
    };

    extraModules = mkOption {
      type = listOf deferredModule;
      default = [];
      description = "Extra modules evaluated inside every `manzil.users.<name>`.";
    };

    specialArgs = mkOption {
      type = attrs;
      default = {};
      description = "Extra specialArgs passed to user submodules.";
    };

    users = mkOption {
      type = attrsOf (submoduleWith {
        modules = [({
    config,
    name,
    options,
    osConfig ? null,
    ...
  }: let
    u = config;
    defaultHome =
      if isDarwin
      then "/Users/${name}"
      else "/home/${name}";
    osHome = osUsers."${name}".home or defaultHome;
    mime = u.xdg.mimeApps;

    mkFileSet = { description, rootDir }:
      mkOption {
        type = attrsOf (({ clobberDefault, rootDir }:
    submodule ({
      config,
      name,
      options,
      ...
    }: let
      generated =
        if config.generator != null
        then config.generator config.value
        else null;
      generatedIsPath = config.generator != null && options.source.type.check generated;
    in {
      options = {
        enable = mkEnableOption "this file" // {default = true;};

        type = mkOption {
          type = enum ["symlink" "copy" "delete" "directory" "modify" "merge"];
          default = "symlink";
          description = "Managed path type.";
        };

        relativeTo = mkOption {
          type = path;
          default = rootDir;
          internal = true;
        };

        target = mkOption {
          type = str;
          default = name;
          defaultText = literalExpression "‹attribute name›";
          apply = p:
            if
              p
              == ""
              || p == "."
              || hasPrefix "/" p
              || any (part: part == "" || part == "." || part == "..") (splitString "/" p)
            then throw "manzil: target must be a clean relative path (got: ${p})"
            else "${config.relativeTo}/${p}";
          description = "Target path, relative to this file set's root.";
        };

        text = mkOption {
          type = nullOr lines;
          default = null;
          description = "Inline contents; becomes a generated source.";
        };

        source = mkOption {
          type = nullOr path;
          default = null;
          description = "Source path for `symlink`/`copy` entries.";
        };

        executable = mkOption {
          type = bool;
          default = false;
          description = "Set +x on generated text sources.";
        };

        clobber = mkOption {
          type = bool;
          default = clobberDefault;
          description = "Overwrite an existing unmanaged target.";
        };

        force = mkOption {
          type = bool;
          default = false;
          description = ''
            Rewrite this entry on every activation, even when the target is
            already managed and byte-identical, for a fresh inode/mtime
            (watcher/daemon invalidation). Orthogonal to `clobber`: clobber
            takes over an unmanaged target; force rewrites an
            already-managed, identical one.
          '';
        };

        permissions = mkOption {
          type = nullOr (strMatching "[0-7]{3,4}");
          default = null;
          description = "Octal mode for `copy`/`directory`/`modify`.";
        };

        uid = mkOption {
          type = nullOr nonnegativeInt;
          default = null;
          description = "Numeric uid for `copy`/`directory`/`modify`.";
        };

        gid = mkOption {
          type = nullOr nonnegativeInt;
          default = null;
          description = "Numeric gid for `copy`/`directory`/`modify`.";
        };

        generator = mkOption {
          type = nullOr (functionTo anything);
          default = null;
          example = literalExpression ''(pkgs.formats.json {}).generate "x.json"'';
          description = "Function applied to `value`; returns a source path or text.";
        };

        value = mkOption {
          type = nullOr anything;
          default = null;
          description = "Argument passed to `generator`; for `merge` entries, the patch attrset.";
        };

        format = mkOption {
          type = nullOr (enum ["json" "toml" "yaml" "ini" "reg"]);
          default = null;
          description = "Format of the existing file a `merge` entry patches into. The patch itself is always JSON.";
        };

        arrayDefault = mkOption {
          type = enum ["replace" "append" "prepend" "union"];
          default = "replace";
          description = "Default array merge strategy for `merge` entries (applies under `clobber = true`).";
        };

        arrays = mkOption {
          type = attrsOf (enum ["replace" "append" "prepend" "union"]);
          default = {};
          example = {"editor.formatters" = "append";};
          description = "Per-path (dot-separated) array strategy overrides for `merge` entries.";
        };
      };

      config = mkMerge [
        (mkIf (config.type == "merge" && config.value != null && config.text == null && config.generator == null) {
          source = mkDefault (pkgs.writeText "manzil-merge-${replaceStrings ["/"] ["-"] name}.json" (toJSON config.value));
        })
        (mkIf (config.text != null) {
          source = mkDefault (pkgs.writeTextFile {
            name = "manzil-${replaceStrings ["/"] ["-"] name}";
            inherit (config) text executable;
          });
        })
        (mkIf generatedIsPath {source = mkDefault generated;})
        (mkIf (config.generator != null && !generatedIsPath && options.text.type.check generated) {
          text = mkDefault generated;
        })
      ];
    })) {
          inherit rootDir;
          clobberDefault = u.clobberByDefault;
        });
        default = {};
        inherit description;
      };
  in {
    options =
      {
        enable = mkEnableOption "manzil for this user" // {default = true;};

        directory = mkOption {
          type = path;
          default =
            if osHome == "/var/empty"
            then defaultHome
            else osHome;
          defaultText = literalExpression "users.users.<name>.home";
          description = "Home directory; root for `files` targets.";
        };

        clobberByDefault = mkOption {
          type = bool;
          default = cfg.clobberByDefault;
          description = "Default `clobber` value for this user's files.";
        };

        packages = mkOption {
          type = listOf package;
          default = [];
          example = literalExpression "[ pkgs.hello ]";
          description = "Packages installed for this user.";
        };

        environment = {
          sessionVariables = mkOption {
            type = attrsOf (nullOr (oneOf [int str path (listOf (oneOf [int str path]))]));
            default = {};
            description = "User session variables; lists become colon-separated values.";
          };

          loadEnv = mkOption {
            type = path;
            readOnly = true;
            description = "Shell script exporting `environment.sessionVariables`.";
          };
        };

        files = mkFileSet {
          rootDir = u.directory;
          description = "Files relative to the user's home directory.";
        };

        xdg = {
          cache.directory = mkOption {
            type = path;
            default = "${u.directory}/.cache";
            defaultText = literalExpression ''"$HOME/.cache"'';
            description = "XDG cache root.";
          };
          cache.files = mkFileSet {
            rootDir = u.xdg.cache.directory;
            description = "Files relative to the XDG cache directory.";
          };

          config.directory = mkOption {
            type = path;
            default = "${u.directory}/.config";
            defaultText = literalExpression ''"$HOME/.config"'';
            description = "XDG config root.";
          };
          config.files = mkFileSet {
            rootDir = u.xdg.config.directory;
            description = "Files relative to the XDG config directory.";
          };

          data.directory = mkOption {
            type = path;
            default = "${u.directory}/.local/share";
            defaultText = literalExpression ''"$HOME/.local/share"'';
            description = "XDG data root.";
          };
          data.files = mkFileSet {
            rootDir = u.xdg.data.directory;
            description = "Files relative to the XDG data directory.";
          };

          state.directory = mkOption {
            type = path;
            default = "${u.directory}/.local/state";
            defaultText = literalExpression ''"$HOME/.local/state"'';
            description = "XDG state root.";
          };
          state.files = mkFileSet {
            rootDir = u.xdg.state.directory;
            description = "Files relative to the XDG state directory.";
          };

          mimeApps = {
            addedAssociations = mkOption {
              type = attrsOf listOrValue;
              default = {};
              description = "MIME Added Associations entries.";
            };
            removedAssociations = mkOption {
              type = attrsOf listOrValue;
              default = {};
              description = "MIME Removed Associations entries.";
            };
            defaultApplications = mkOption {
              type = attrsOf listOrValue;
              default = {};
              description = "MIME Default Applications entries.";
            };
          };
        };
      }
      // optionalAttrs isNixOS {systemd = listToAttrs (map (t:
      nameValuePair "${t}s" (mkOption {
        type = utils.systemdUtils.types."${t}s";
        default = {};
        description = "User systemd ${t} units.";
      }))
    unitTypes)
    // {
      enable = mkEnableOption "user systemd unit generation" // {default = true;};

      units = mkOption {
        type = utils.systemdUtils.types.units;
        default = {};
        internal = true;
        description = "Generated user systemd units.";
      };
    };};

    config = mkMerge [
      {
        environment = {
          sessionVariables = {
            XDG_CACHE_HOME = mkIf (u.xdg.cache.directory != options.xdg.cache.directory.default) u.xdg.cache.directory;
            XDG_CONFIG_HOME = mkIf (u.xdg.config.directory != options.xdg.config.directory.default) u.xdg.config.directory;
            XDG_DATA_HOME = mkIf (u.xdg.data.directory != options.xdg.data.directory.default) u.xdg.data.directory;
            XDG_STATE_HOME = mkIf (u.xdg.state.directory != options.xdg.state.directory.default) u.xdg.state.directory;
          };
          loadEnv = mkDefault ((name: env:
    pkgs.writeShellScript "manzil-env-${name}" (
      pipe env [
        (filterAttrs (_: v: v != null))
        (mapAttrsToList (k: v: "export ${k}=${escapeShellArg ((v:
    if isList v
    then concatMapStringsSep ":" toString v
    else toString v) v)}"))
        (concatStringsSep "\n")
      ]
    )) name u.environment.sessionVariables);
        };
      }

      (mkIf (mime.addedAssociations != {} || mime.removedAssociations != {} || mime.defaultApplications != {}) {
        xdg.config.files."mimeapps.list".text =
          mimeSection "Added Associations" mime.addedAssociations
          + mimeSection "Removed Associations" mime.removedAssociations
          + mimeSection "Default Applications" mime.defaultApplications;
      })

      (optionalAttrs isNixOS {
        systemd.units = pipe unitTypes [
          (map (t:
            mapAttrsToList
            (n: v: nameValuePair "${n}.${t}" (utils.systemdUtils.lib."${t}ToUnit" v))
            u.systemd."${t}s"))
          flatten
          listToAttrs
        ];
      })

      (optionalAttrs isNixOS (mkIf (u.systemd.enable && u.systemd.units != {}) {
        xdg.config.files."systemd/user".source = utils.systemdUtils.lib.generateUnits {
          type = "user";
          inherit (u.systemd) units;
          inherit (osConfig.systemd) package;
          packages = [];
          upstreamUnits = [];
          upstreamWants = [];
        };
      }))
    ];
  })] ++ cfg.extraModules;
        specialArgs =
          cfg.specialArgs
          // {
            inherit pkgs;
            osConfig = config;
            osOptions = options;
          }
          // optionalAttrs isNixOS {inherit utils;};
      });
      default = {};
      description = "Per-user configurations, keyed by username.";
    };
  }
  // optionalAttrs isFinix {
    finit.conditions = mkOption {
      type = listOf str;
      default = [];
      example = ["task/persist-user-binds/success"];
      description = ''
        When non-empty, run the linker as a `finit` task gated on these
        conditions instead of a `system.activation` script. Use for homes
        that only exist after activation (late mounts, impermanence binds).
      '';
    };
  };

  config = mkIf (enabledUsers != {}) (mkMerge [
    {
      assertions = flatten (mapAttrsToList (uname: u:
        optional (isNixOS || isFinix) (let
          osUser = osUsers."${uname}" or {};
        in {
          assertion =
            (osUser.enable or false)
            && ((osUser.isNormalUser or false) || (osUser.isSystemUser or false) || (osUser.home or null) != "/var/empty");
          message = "manzil.users.${uname}: enabled user must exist and be enabled in users.users.";
        })
        ++ [
          (let
            targets = map (f: f.target) (filter (f: f.enable) (allEntries u));
          in {
            assertion = length targets == length (unique targets);
            message = "manzil.users.${uname}: duplicate target paths are not allowed.";
          })
        ]
        ++ concatMap (set: (uname: label: files:
    mapAttrsToList (key: f: let
      wantsSource = elem f.type ["symlink" "copy" "merge"];
      isMerge = f.type == "merge";
    in {
      assertion =
        !f.enable
        || (
          (
            if wantsSource
            then f.source != null
            else f.source == null
          )
          && (!(f.permissions != null || f.uid != null || f.gid != null) || elem f.type ["copy" "directory" "modify" "merge"])
          && (isMerge -> (f.format != null))
          && (!isMerge -> (f.format == null && f.arrays == {}))
        );
      message =
        "manzil.users.${uname}.${label}.\"${key}\": "
        + (
          if wantsSource && f.source == null
          then "`source`, `text`, `value`, or `generator` is required for `${f.type}`."
          else if !wantsSource && f.source != null
          then "`source`, `text`, and source-valued `generator` are only valid for symlink/copy/merge."
          else if isMerge && f.format == null
          then "`format` is required for merge entries."
          else if !isMerge && (f.format != null || f.arrays != {})
          then "`format` and `arrays` are only valid for merge entries."
          else "metadata is only valid for copy/directory/modify/merge."
        );
    })
    files) uname set.label set.files) (fileSets u))
      enabledUsers);

      users.users = mapAttrs (_: u: {inherit (u) packages;}) (filterAttrs (_: u: u.packages != []) enabledUsers);
      environment.systemPackages = [cfg.linker];
    }

    (optionalAttrs isNixOS {
      system.activationScripts.manzil = stringAfter ["users" "groups"] activationScript;
    })

    (optionalAttrs isDarwin {
      system.activationScripts.manzil.text = activationScript;
    })

    (optionalAttrs isFinix {
      # Runs every boot (finix-setup) and on every switch, after the users
      # script; the manifest rides the topLevel closure, so every source it
      # references stays GC-rooted.
      system.activation.scripts.manzil = mkIf (cfg.finit.conditions == []) {
        deps = ["users"];
        text = activationScript;
      };
      finit.tasks.manzil = mkIf (cfg.finit.conditions != []) {
        description = "manzil: materialize user files";
        conditions = cfg.finit.conditions;
        command = pkgs.writeShellScript "manzil-link" activationScript;
        log = true;
      };
    })
  ]);
}
