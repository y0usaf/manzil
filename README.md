# manzil

> منزل — *home* (ar.)

A deliberately tiny replacement for Home Manager's `home.files`. A NixOS
module plus a small Rust linker (`manzil`) — that's the entire project.

## Scope

manzil only does what `home.files` does, no more:

- Declare files in a user's `$HOME` from Nix.
- Link them with symlinks to `/nix/store`.
- Track them across rebuilds via a JSON manifest, so removed entries are
  cleaned up (no dangling symlinks).

It does **not** ship XDG helpers, generators, format writers, systemd unit
generation, mime-apps, environment variables, package management, or
per-platform abstractions. Layer those on top yourself if you want them.

## Usage

```nix
# flake.nix
{
  inputs.manzil.url  = "github:y0usaf/Manzil";
  inputs.nixpkgs.url = "...";

  outputs = { nixpkgs, manzil, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      modules = [
        manzil.nixosModules.default
        ({ pkgs, lib, ... }: {
          users.users.alice = { isNormalUser = true; home = "/home/alice"; };

          manzil.users.alice = {
            # ── relative to $HOME ──────────────────────────────────────
            files.".bashrc".text = ''
              alias ll='ls -la'
            '';

            files.".local/bin/script" = {
              source     = ./bin/script.sh;
              executable = true;
            };

            files.".gitconfig" = {
              source  = ./gitconfig;
              clobber = true;            # overwrite real files
            };

            # ── relative to $XDG_CONFIG_HOME (~/.config) ───────────────
            xdg.config.files."foo/foo.toml".source = ./foo.toml;

            # generator: result is a derivation → goes to `source`
            xdg.config.files."app/config.json" = {
              generator = (pkgs.formats.json { }).generate "config.json";
              value     = { theme = "dark"; size = 14; };
            };

            # generator: result is a string → goes to `text`
            xdg.config.files."git/config" = {
              generator = lib.generators.toGitINI;
              value     = { user = { email = "me@me.tld"; name = "Me"; }; };
            };

            # ── other XDG roots ────────────────────────────────────────
            xdg.cache.files."app/seed".text  = "deadbeef";
            xdg.data.files."app/db.sqlite".source = ./seed.db;
            xdg.state.files."app/init".text  = "";
          };
        })
      ];
    };
  };
}
```

## Options

### Top-level

| Option                                  | Type   | Default | Notes                                |
| --------------------------------------- | ------ | ------- | ------------------------------------ |
| `manzil.clobberByDefault`               | bool   | `false` | Default `clobber` for every file.    |
| `manzil.users.<name>.enable`            | bool   | `true`  | Skip the user's files when `false`.  |
| `manzil.users.<name>.directory`         | path   | `users.users.<name>.home` | Root for `files`. |
| `manzil.users.<name>.clobberByDefault`  | bool   | `manzil.clobberByDefault` | Per-user override. |

### File sets (five per user)

Each of these is an `attrsOf` file entries (see below):

| Option                                | Default root            |
| ------------------------------------- | ----------------------- |
| `manzil.users.<name>.files`           | `directory` (`$HOME`)   |
| `manzil.users.<name>.xdg.cache.files` | `$HOME/.cache`          |
| `manzil.users.<name>.xdg.config.files`| `$HOME/.config`         |
| `manzil.users.<name>.xdg.data.files`  | `$HOME/.local/share`    |
| `manzil.users.<name>.xdg.state.files` | `$HOME/.local/state`    |

Each root is overridable via the matching `xdg.<x>.directory` option. Note
that manzil does **not** export `XDG_*_HOME` environment variables — set
those yourself in `environment.sessionVariables` or your shell init if you
override a root.

### File entry options

| Option       | Type                  | Default              | Notes |
| ------------ | --------------------- | -------------------- | ----- |
| `enable`     | bool                  | `true`               | Skip when `false`. |
| `target`     | str (relative)        | the attribute name   | Resolved against the file set's root. Absolute paths rejected. |
| `text`       | str?                  | `null`               | Inline contents. Becomes a `writeTextFile` source. |
| `source`     | path?                 | `null`               | Source file or directory to symlink. |
| `executable` | bool                  | `false`              | +x bit (only meaningful with `text` or string `generator`). |
| `clobber`    | bool                  | `clobberByDefault`   | Overwrite an existing non-symlink target. |
| `generator`  | function?             | `null`               | Applied to `value`; result routes to `source` (path/derivation) or `text` (string). |
| `value`      | any?                  | `null`               | Argument passed to `generator`. |

## How it works

1. The module renders one JSON manifest per user (`/nix/store/...-manzil-manifest-<user>.json`).
2. A NixOS activation script, ordered after `users`/`groups`:
   - Runs the `manzil` binary as the target user via `runuser`.
   - The binary compares the new manifest with the previous one in `/var/lib/manzil/`
     and reconciles symlinks: prunes removed entries, links new ones, refreshes
     changed ones.
   - On linker success, the activation script copies the new manifest into
     `/var/lib/manzil/manifest-<user>.json` for the next rebuild. On failure
     state is left untouched so the next run can retry.

### Robustness properties

- **Atomic per-file replacement.** New symlinks are created at a sibling
  `.manzil-tmp.<pid>` path and `rename(2)`d into place. No ENOENT window for
  readers.
- **Mutual exclusion.** `flock(2)` on `$HOME/.local/state/manzil/lock` is held
  for the duration of a run; concurrent invocations serialise instead of
  racing.
- **Idempotent.** A re-run with an unchanged manifest performs zero writes and
  emits zero output.
- **Continue-on-error.** Per-entry failures are reported on stderr; the run
  exits non-zero only if any entry failed, but every other entry is still
  attempted.
- **No runtime dependencies** beyond `glibc`. The binary is ~370 KiB.

### Manifest schema

```json
{
  "files": [
    { "target": "/home/alice/.bashrc",     "source": "/nix/store/...", "clobber": false },
    { "target": "/home/alice/.gitconfig",  "source": "/nix/store/...", "clobber": true  }
  ]
}
```

### Linker rules

- Target in old manifest, not in new → removed iff it's still a symlink.
  User-replaced regular files are left alone (with a warning).
- Target in new manifest:
  - Existing symlink to the correct source → no-op (silent).
  - Existing symlink to a different source → atomic swap.
  - Existing non-symlink + `clobber=true`  → removed and replaced.
  - Existing non-symlink + `clobber=false` → warned, skipped.
  - Missing → created.

You can invoke the linker directly:

```sh
manzil /path/to/new.json /var/lib/manzil/manifest-alice.json
```

## Non-goals

- Multi-platform (Darwin, etc.). NixOS only.
- Reload/restart of user services on file change — out of scope; wire it up
  yourself with `systemd.user.services.<x>.restartTriggers` if you need it.
- Per-file uid/gid/perms beyond `executable` — symlinks don't carry mode, and
  the user owns their own home.

## Building the linker

```sh
nix build .#manzil           # via flake
cargo build --release        # directly
```

The binary depends only on `serde`, `serde_json`, and `libc`.

## License

MIT. The whole thing is ~350 lines: ~155 nix + ~190 rust.
