# Downstream overlay (StGit)

This checkout carries a small set of local changes (mainly the Tauri desktop
app) on top of upstream [gingko/client](https://github.com/gingko/client).
The changes are kept as a [StGit](https://stacked-git.github.io) patch stack
so they can always be replayed onto the latest upstream `master`.

This file is itself the first patch of the stack (`overlay-workflow`).

## Branches

| branch    | contents                                                       |
|-----------|----------------------------------------------------------------|
| `master`  | follows `upstream/master` exactly, never committed to |
| `overlay` | upstream `master` + the StGit patch stack, the branch to build |

Remotes: `upstream` is `gingko/client`, `origin` is the personal fork
(`gyger/gingko`), where the overlay is published.

## Updating to the latest upstream

```bash
git switch overlay
git fetch upstream
git switch master && git merge --ff-only upstream/master && git switch overlay
stg rebase upstream/master    # pops all patches, moves base, re-pushes them
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

## Publishing to GitHub (and restoring the stack elsewhere)

A plain `git push` / `git clone` only carries the `overlay` branch, i.e. the
patches as ordinary commits. StGit's own metadata (patch names, which patches
are applied, the stack's operation log) lives in a separate ref,
`refs/stacks/overlay`, a commit containing `stack.json`. Sync it explicitly
next to the branch; GitHub stores it like any other ref (it's just not shown
in the web UI):

```text
origin (gyger/gingko)
├── refs/heads/overlay     ← the code: upstream master + patches as commits
└── refs/stacks/overlay    ← StGit metadata for that branch
```

The per-patch refs `refs/patches/overlay/*` don't need syncing; StGit
recreates them from `refs/stacks/overlay`.

### Push

```bash
git push --force-with-lease origin overlay   # branch is rewritten by rebases
git push origin refs/stacks/overlay          # no force needed, see below
```

Every StGit operation appends a commit to the stack ref, so its history is
linear and a normal push fast-forwards. If that push is rejected, the stack
was changed somewhere else in the meantime. Don't force it; fetch the
remote stack and reconcile first.

Optional alias for both steps:

```bash
git config alias.overlay-push '!git push --force-with-lease origin overlay && git push origin refs/stacks/overlay'
```

### Set up a new checkout

Remotes, branches and the stack metadata; branch config such as
`branch.overlay.*` is local git config and doesn't travel with the repo:

```bash
git clone -b overlay https://github.com/gyger/gingko.git gingko && cd gingko
git branch --unset-upstream                  # overlay tracks nothing
git remote add upstream https://github.com/gingko/client.git
git fetch upstream master
git branch --track master upstream/master
git fetch origin refs/stacks/overlay:refs/stacks/overlay
stg series                                   # should list the patches
```

The stack ref and the branch have to belong together: the stack's recorded
head must equal the `overlay` commit. They do as long as both were pushed
together. If `stg` reports that the branch was modified outside StGit, run
`stg repair`.

To pick up stack changes pushed from another machine later (this discards
local, unpushed work on `overlay`):

```bash
git fetch origin
git switch overlay && git reset --hard origin/overlay
git fetch origin +refs/stacks/overlay:refs/stacks/overlay
stg series
```

Deliberately **not** used: a permanent `fetch = +refs/stacks/*:refs/stacks/*`
refspec on `origin`. It would force-overwrite the local stack metadata on
every `git fetch`, even when you have unpushed StGit work, and leave it out
of sync with the local `overlay` branch.

### Plain patch files

`stg export -d patches/` writes the stack as plain patch files, e.g. for
review or for applying with `git am` elsewhere.
