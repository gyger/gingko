# Downstream overlay (StGit)

This checkout carries a small set of local changes (mainly the Tauri desktop
app) on top of upstream [gingko/client](https://github.com/gingko/client).
The changes are kept as a [StGit](https://stacked-git.github.io) patch stack
so they can always be replayed onto the latest upstream `master`.

This file is itself the first patch of the stack (`overlay-workflow`).

## Branches

| branch    | contents                                                       |
|-----------|----------------------------------------------------------------|
| `master`  | follows upstream `origin/master` exactly, never committed to   |
| `overlay` | upstream `master` + the StGit patch stack, the branch to build |

The upstream remote here is `origin` (`gingko/client`). A personal fork can
be added as a second remote for publishing the overlay (see below).

## Updating to the latest upstream

```bash
git switch overlay
git fetch origin
git switch master && git merge --ff-only origin/master && git switch overlay
stg rebase origin/master      # pops all patches, moves base, re-pushes them
```

If a patch no longer applies, `stg rebase` stops at it with conflict markers:

```bash
# fix the files, then
git add <files>
stg refresh                   # record the resolution in the current patch
stg push --all                # continue with the remaining patches
```

Then verify the build (from `client/`):

```bash
bun i
cp config-example.js config.js      # once
bun run tauri:frontend              # elm make + esbuild + tailwind
bun run tauri:dev                   # or tauri:build for installers
```

`client/bun.lockb` is binary and conflicts whenever upstream changes
dependencies. Don't try to merge it: take upstream's version
(`git checkout --ours client/bun.lockb` during a StGit conflict), run
`bun install --lockfile-only` in `client/` so it picks up the Tauri
dependencies from `client/package.json` again, and `stg refresh` the result
into the patch that added those dependencies (`tauri-desktop-app`).

## Patches are intentions, not diffs

Each patch message says **why** the change exists and what it must keep
working, not only what lines it touches. When upstream restructures the code a
patch touches, reapplying the old hunks is often the wrong fix. Read the
patch description and reimplement the intent against the new upstream code,
then `stg refresh`. Update the description when the implementation changes.

When writing or editing a patch description, cover:

- **Purpose**: what user-visible behaviour or capability the patch provides.
- **Approach**: the key design decisions, the ones a reimplementation needs.
- **Constraints**: what must not change (upstream web app behaviour, file
  formats, public Elm module APIs used by the web app).

`stg edit <patch>` edits a patch's message; `stg show <patch>` shows it.

## Working on the stack

```bash
stg series --description      # list patches with their subjects
stg new <name>                # start a new patch on top
stg refresh                   # fold working-tree changes into the top patch
stg refresh -p <name>         # ... or into a specific patch
stg goto <name>               # pop/push until <name> is on top
stg squash / stg pick / stg delete
```

Keep patches focused: bug fixes to an existing overlay feature can either
stay as their own patch (documents the history of the fix) or be squashed
into the feature patch once they are no longer interesting on their own.

Upstream the patches that could be generally useful.
If upstream merges an equivalent change, `stg rebase` reports the patch as
empty; remove it with `stg delete`.

## Publishing

The `overlay` branch is rewritten on every rebase, so publishing to a fork
needs a force push:

```bash
git remote add fork https://github.com/gyger/ginko-client.git   # once
git push --force-with-lease fork overlay
```

`stg export -d patches/` writes the stack as plain patch files, e.g. for
review or for applying with `git am` elsewhere.
