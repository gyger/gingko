# taudesktop → upstream monorepo migration

Status of this branch, and what still needs doing on a machine with the real
toolchains. Written 2026-09-07.

## What upstream did

On 2026-09-04 upstream (`gingko/client`) folded the separate `gingko/server`
repository into the client repository as a monorepo:

| commit    | change                                                          |
|-----------|-----------------------------------------------------------------|
| `8b08cdc` | pure rename: all 432 client files → `client/`                   |
| `9607d01` | pure rename inside the `gingko/server` history → `server/`      |
| `9f61bfc` | merge commit joining the two previously-unrelated histories     |
| `b78fc5b` | new root `.github/workflows/web-deploy.yml`                     |

No client code changed in the move itself (0 insertions, 0 deletions). The
reason shows up in the 16 commits that follow: the Cypress → Playwright e2e
migration. `client/tests/e2e/base.ts` now spawns a **real server per spec
file** — `node ../server/dist/index.js` with `TEST_DB_PATH` / `PORT` /
`E2E_NO_EMAIL`, against a throwaway copy of a committed SQLite fixture in
`client/tests/e2e/fixtures/db/`, each worker on its own port. That
cross-tree `../server` reference only works with both halves in one checkout.
Secondary benefit: one pipeline that builds and deploys both halves together.

Two side effects:

- **Only the root `.github/workflows` is live.**
  `client/.github/workflows/build.yml` (the Electron release build) and
  `server/.github/workflows/server-deploy.yml` still exist in-tree but GitHub
  never reads them — dead leftovers of the move.
- The root `.gitignore` is new and minimal (`/data/`, `.idea/`); the real
  ignore rules live in `client/.gitignore`.

## What this branch is

`taudesktop` (`822b261`, 12 commits on top of `5db9854` from 2026-04-03)
merged with upstream `master` (`b16af93`, 20 commits ahead of that fork
point).

The merge was done with `git merge`, **not** rebase: git's rename detection
resolves the whole 432-file relocation automatically and re-applies every
`taudesktop` edit onto the new `client/…` paths. Only `.gitignore` conflicted.

Two manual fix-ups were needed, because git relocates *modified* files but
leaves *newly added* ones where they were:

1. Root `.gitignore` — took upstream's version; `tauri-web/` and
   `src-tauri/target/` moved into `client/.gitignore`.
2. `git mv` of the Tauri files git had stranded at the repo root:
   `esbuild-tauri.mjs`, `src-tauri/`, `src/tauri/`, `tests/historic/`
   → all under `client/`.

Nothing inside `src-tauri/` or `esbuild-tauri.mjs` needed editing:
`esbuild-tauri.mjs` resolves everything from `import.meta.dirname`, and
`tauri.conf.json`'s `../tauri-web` and `../build/icon.ico` still resolve
correctly now that `src-tauri/` sits inside `client/`.

## Verified

- Merged tree is exactly upstream `b16af93` plus the *identical* 38-file /
  13,536-insertion / 147-deletion `taudesktop` delta, now under `client/`.
- `client/src/elm/Page/Doc.elm` auto-merged: the merged file carries both
  upstream's `cardsCollapsed` / `"z"` collapsed-card handler and the desktop
  `SaveRequested` … `FileChangedOnDisk` cases.
- `ScrollCards` arity is consistent — upstream widened it to 5 args and
  updated both Elm call sites; the desktop Elm code never constructs it.
- `client/package.json` kept the `tauri:*` scripts and both `@tauri-apps`
  deps; `client/README.md` kept the Tauri section.

## NOT verified — do this first locally

No Elm and no Rust toolchain existed in the environment where this merge was
made, so **nothing was compiled**. The merge is textually coherent and
arity-consistent, that's all.

```bash
cd client
bun i
ELM_HOME=elm-home/elm-stuff bun run tauri:frontend   # elm make + esbuild + tailwind
bun run tauri:dev                                    # cargo build + run the app
```

Then the web build, which is the path upstream actually changed
(`elm-postprocess.mjs` was rewritten in `059b71b`):

```bash
cd client
cp config-example.js config.js        # or config_decrypt.sh
bun run newbuild
```

## Then: the e2e suite needs a built server sibling

This is the one genuinely new requirement the monorepo imposes on us.
Without `server/dist/index.js`, every Playwright spec fails at server startup.

```bash
cd server
cp config-example.js config.js        # placeholder values are fine for tests
npm i                                 # better-sqlite3 native build; needs node, not bun
npx tsc
cd ../client && bun run test
```

The Tauri app itself is unaffected — it is local-file-only and never talks to
`server/`.

## Follow-ups (not done here, out of scope of the merge)

- **`z` is missing from the desktop shortcut tray.** Upstream added the
  collapsed-card shortcut to `client/src/elm/Doc/HelpScreen.elm` and
  `client/src/elm/Doc/UI.elm`, but not to
  `client/src/elm/Electron/ShortcutsModal.elm`, which is a file this branch
  owns. One-line addition.
- **Desktop CI must live at the repo root.** Any Tauri build workflow needs
  to be `.github/workflows/*.yml` with `working-directory: client`; a
  workflow under `client/.github/workflows/` will silently never run. The
  existing `client/.github/workflows/build.yml` is the old Electron
  release job and is currently inert.
- Decide what to do with `client/app/` and the Electron scripts in
  `client/package.json` now that the Tauri app supersedes them.

## Keeping up with upstream from here on

```bash
git remote add upstream https://github.com/gingko/client   # if not already
git fetch upstream master
git merge upstream/master
```

Future merges should be uneventful — the one-time path rewrite is behind us.
