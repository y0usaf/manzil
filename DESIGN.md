# manzil — DESIGN

## Locked decisions

- **2026-07-30 — absorb patchix as a `merge` file entry type.** manzil and
  patchix are always co-deployed by the same sole maintainer; two repos meant
  two flake locks, two release cadences, and an activation-ordering race
  (patchix ran at `multi-user.target`, after linking). Merging deletes the
  patchix repo: its flake input, NixOS module (~270 lines), per-user systemd
  oneshot, and cross-format CLI plumbing. Rejected smaller thing
  ([[canon:least-code]]): a `manzil.extraModules` shim generating patchix
  units (~100 lines) — kept both repos alive forever for no capability gain.
- **2026-07-30 — patch payloads are always JSON in the Nix store.** Nix
  attrsets are JSON-complete; the `format` field describes only the
  *existing* file being merged into. Kills `--patch-format`, format
  autodetection, and `pkgs.formats.{toml,yaml}` patch generation.
- **2026-07-30 — all five existing-file formats move: json, toml, yaml, ini,
  reg.** Owner decision, patchix parity over minimalism; reg serves a live
  wine use. Divergence from least-code (yaml/ini currently have zero users;
  reg.rs is 1578 lines for one game) is accepted and recorded here. Reversal
  condition: a format with zero users for a year gets deleted.
- **2026-07-30 — merge entries prune by un-merge against the old patch.**
  manzil's invariant is verifiable ownership: prune only what provably came
  from the manifest (`prune_symlink` checks the link target, `prune_copy`
  compares bytes). A removed/changed merge entry un-merges each leaf key
  whose current disk value equals the **old** patch value; user-edited keys
  stay; fail-soft. Documented irreversibles: RFC 7396 null-deletes,
  append/prepend/union array elements, GC'd old patch source (warn + skip —
  same exposure `prune_copy` already has).
- **2026-07-30 — `clobber` on a merge entry reuses manzil's existing field
  and default.** manzil defaults `clobber = false` (fill missing keys only);
  patchix defaulted `true` (overwrite). manzil's safer default wins.
  Migrating patches that relied on overwrite must set `clobber = true`
  explicitly.
- **2026-07-30 — impermanence is not absorbed, at either level.**
  System-level (tmpfs root, subvol mounts, `neededForBoot` fstab binds,
  `/etc`, machine-id) is pre-activation epoch: it must exist before manzil's
  `stringAfter ["users"]` slot can run, and it is already pure data in
  fstab/`fileSystems` — a JSON-manifest replay would be more code and more
  power for the same mounts. User-home bind mounts were considered as a
  `bind` entry type with a `--phase root` pass and rejected: it deletes ~52
  lines of a working finit task in exchange for `mount(2)` behind a manifest
  parser holding CAP_SYS_ADMIN inside the shared linker binary. Composition
  stays one string: `manzil.finit.conditions = ["task/persist-user-binds/success"]`.
- **2026-08-02 — per-entry `force` rewrites even unchanged entries.**
  `force = true` bypasses the three skip guards (symlink already-pointing,
  copy same-contents, merge byte-identical output) and rewrites via the
  existing atomic sibling-tmp+rename paths, so the target gets a fresh
  inode/mtime every activation (watcher/daemon invalidation). Orthogonal to
  `clobber`: clobber takes over an UNMANAGED target; force rewrites an
  already-managed-and-identical one. Both refuse directory targets. Rejected
  alternatives: extending `clobber` (two orthogonal intents in one bool) and
  a `forceByDefault` default (the second use does not exist yet — wait for
  one). Schema v3 gains the optional `force` field; module and linker ship
  from the same repo, so old-binary/new-manifest ambiguity cannot occur in
  practice.

## Architecture

| module | role | kind |
|---|---|---|
| `nix/modules/common.nix` | option surface, manifest generation, activation script, assertions | decision-making |
| `nix/modules/{nixos,darwin,finix}.nix` | platform adapters | machinery |
| `src/main.rs`, `src/app.rs` | argv, old/new manifest diff loop | machinery |
| `src/manifest.rs` | schema v3 parse + validation | machinery |
| `src/filesystem.rs` | activate/prune per entry type | machinery |
| `src/merge.rs` (moved from patchix) | deep merge, array strategies, un-merge | machinery |
| `src/formats/` (moved from patchix) | json/toml/yaml/ini/reg read+write | machinery |

Extension surface is `manzil.extraModules` + `specialArgs` + the swappable
`manzil.linker` package; there is no plugin story beyond that and none is
planned ([[canon:no-privileged-path]] n/a — the API would exceed the feature
set; reversed if a third external linker consumer appears).

Manifest schema v3: v2 plus entry type `merge` with fields `source` (JSON
patch, store path), `format` (`json|toml|yaml|ini|reg`), `clobber`,
`arrayDefault` (`replace|append|prepend|union`), `arrays` (dot-path →
strategy), optional `permissions`/`uid`/`gid`, and optional `force`
(rewrite even byte-identical managed entries). Merge targets are created if
missing, written via atomic sibling-tmp rename, never owned: dropping the
entry triggers un-merge, not file deletion.

## Deferred

- **`bind` entry type + privileged linker phase** (user-home persist binds,
  manifest-diffed convergence — the current finit task never unmounts
  dropped entries). Revisit if a second host hand-replays the bind task or a
  stale-mount bug actually bites.
- **yaml/ini format deletion.** Kept at owner's decision despite zero
  current users; see reversal condition above.
- **patchix repo archive** — after manzil v3 ships and `~/nixos` migrates
  its three patches (kimi-code toml, codex toml, solo-leveling reg).
- **`merge` un-merge for arrays** — elements are unattributable after
  append/union; no design exists that doesn't require a snapshot store.

## Roadmap

1. **Move code.** `merge.rs` + `formats/` into manzil crate; deps `toml`,
   `serde_yml`, `rust-ini`, `anyhow` added. Criterion: `nix build` passes
   with patchix tests relocated and green.
2. **Schema v3.** `manifest.rs` gains `merge` entry + validation
   (`format` only on merge, `source` required, arrays fields only on merge).
   Criterion: v2 manifests still parse; malformed merge entries rejected
   loudly in tests.
3. **Activate + un-merge prune.** `filesystem.rs` dispatch; old-patch
   un-merge (~70 lines). Criterion: nix-build test — add entry, merge
   appears; edit key on disk, re-run, edit survives (no-clobber); remove
   entry, unedited keys vanish, edited keys stay.
4. **Module surface.** `type = "merge"` in the file-set submodule: `format`,
   `arrayDefault`, `arrays` options; `value` serialized via `writeText` +
   `toJSON`; assertions. Criterion: `nix flake check`; example in README
   evaluates.
5. **Migrate `~/nixos`.** Three patches become manzil entries with explicit
   `clobber`; patchix input/module/service removed. Criterion: `nh os
   switch` on both hosts; merged files byte-identical to pre-migration
   output.
6. **Archive patchix.** README pointer to manzil. Criterion: repo archived,
   no flake references remain in `~/nixos`.
