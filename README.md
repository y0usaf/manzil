# manzil

> منزل — *home* (ar.)

Tiny home management for NixOS + nix-darwin: a small module, a small Rust
linker, JSON manifests.

## Features

- Per-user home files under `$HOME` + XDG roots.
- File entry types: `symlink`, `copy`, `delete`, `directory`, `modify`.
- Metadata for real paths: `permissions`, `uid`, `gid`.
- Per-user environment scripts + automatic `XDG_*_HOME` when XDG roots change.
- `mimeapps.list` generation.
- Per-user packages.
- NixOS user systemd unit generation.
- nix-darwin module export.
- Configurable linker package + args.
- User submodule extension via `extraModules` + `specialArgs`.

## Usage

```nix
# flake.nix
{
  inputs.manzil.url = "github:you/manzil";

  outputs = { self, nixpkgs, manzil, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      modules = [
        manzil.nixosModules.default
        ({ pkgs, config, ... }: {
          users.users.alice = { isNormalUser = true; home = "/home/alice"; };

          manzil.users.alice = {
            packages = [ pkgs.hello ];
            environment.sessionVariables.EDITOR = "nvim";

            files.".manzil-env".source = config.manzil.users.alice.environment.loadEnv;
            files.".bashrc".text = ''
              export MANZIL=1
              . ~/.manzil-env
            '';

            files.".config/app/config.toml" = {
              type = "copy";
              text = "theme = 'dark'\n";
              permissions = "0600";
            };

            files.".cache/app" = {
              type = "directory";
              permissions = "0700";
            };

            files."old-file".type = "delete";
            files.".ssh/config" = { type = "modify"; permissions = "0600"; };

            xdg.mimeApps.defaultApplications."text/plain" = "nvim.desktop";

            systemd.services.demo.serviceConfig.ExecStart = "${pkgs.hello}/bin/hello";
          };
        })
      ];
    };
  };
}
```

For nix-darwin, import `manzil.darwinModules.default`.

## Options

### Top-level

| Option | Type | Default |
| --- | --- | --- |
| `manzil.clobberByDefault` | bool | `false` |
| `manzil.linker` | package | bundled `manzil` |
| `manzil.linkerArgs` | list of string | `[ ]` |
| `manzil.extraModules` | list of modules | `[ ]` |
| `manzil.specialArgs` | attrs | `{ }` |
| `manzil.users.<name>.enable` | bool | `true` |
| `manzil.users.<name>.directory` | path | OS user home |
| `manzil.users.<name>.packages` | list of packages | `[ ]` |
| `manzil.users.<name>.environment.sessionVariables` | attrs | `{ }` |
| `manzil.users.<name>.environment.loadEnv` | path | generated shell script |

### File sets

All are `attrsOf` file entries:

| Option | Root |
| --- | --- |
| `manzil.users.<name>.files` | `$HOME` |
| `manzil.users.<name>.xdg.cache.files` | `$HOME/.cache` |
| `manzil.users.<name>.xdg.config.files` | `$HOME/.config` |
| `manzil.users.<name>.xdg.data.files` | `$HOME/.local/share` |
| `manzil.users.<name>.xdg.state.files` | `$HOME/.local/state` |

Each XDG root has a matching `.directory` option. If changed, the generated env
script exports the corresponding `XDG_*_HOME` variable.

### File entries

| Option | Type | Default | Notes |
| --- | --- | --- | --- |
| `enable` | bool | `true` | Skip when false. |
| `type` | enum | `"symlink"` | `symlink`, `copy`, `delete`, `directory`, `modify`. |
| `target` | clean relative string | attr name | Absolute paths, `.`, `..`, empty segments rejected. |
| `text` | string? | `null` | Generated source for `symlink`/`copy`. |
| `source` | path? | `null` | Required for `symlink`/`copy`. |
| `executable` | bool | `false` | +x for generated `text` source. |
| `clobber` | bool | default | Replace unmanaged targets where safe. |
| `permissions` | octal string? | `null` | For `copy`/`directory`/`modify`. |
| `uid` / `gid` | int? | `null` | For `copy`/`directory`/`modify`. |
| `generator` | function? | `null` | Applied to `value`; returns source path or text. |
| `value` | any? | `null` | Generator input. |

## Linker rules

Manifest schema v2:

```json
{
  "version": 2,
  "files": [
    { "type": "symlink", "target": "/home/alice/.bashrc", "source": "/nix/store/...", "clobber": false },
    { "type": "copy", "target": "/home/alice/.config/app", "source": "/nix/store/...", "permissions": "0600" },
    { "type": "directory", "target": "/home/alice/.cache/app", "permissions": "0700" },
    { "type": "delete", "target": "/home/alice/old" },
    { "type": "modify", "target": "/home/alice/.ssh/config", "permissions": "0600" }
  ]
}
```

- `symlink`: current behavior; owned symlinks update/prune atomically.
- `copy`: copies a source file. Existing files update only when unchanged from the old source or `clobber = true`.
- `directory`: creates a real directory; prunes only if empty.
- `delete`: removes a file/symlink/directory tree if present.
- `modify`: applies metadata to an existing path; missing paths are ignored.
- Symlinks never receive `permissions`/`uid`/`gid`.

Invoke directly:

```sh
manzil /path/to/new.json /path/to/old.json
```

## Systemd user units, MIME, env

```nix
manzil.users.alice = {
  environment.sessionVariables.PATH = [ "$HOME/.local/bin" "/run/current-system/sw/bin" ];

  xdg.mimeApps = {
    defaultApplications."text/plain" = [ "nvim.desktop" ];
    addedAssociations."image/png" = [ "imv.desktop" "gimp.desktop" ];
  };

  systemd.services.agent = {
    description = "demo agent";
    serviceConfig.ExecStart = "${pkgs.hello}/bin/hello";
  };
};
```

`environment.loadEnv` is a POSIX shell script; source it from your shell/profile
where desired.

## Building

```sh
nix build
cargo build --release
```
