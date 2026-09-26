![](./client/docs/images/screenshot-alien-screenplay.png)

# Gingko Writer [![Web Deploy](https://github.com/gingko/client/actions/workflows/web-deploy.yml/badge.svg)](https://github.com/gingko/client/actions/workflows/web-deploy.yml)

Writing software to help organize and draft complex documents. Anything from novels and screenplays to legal briefs and graduate theses.

This is a ground-up rewrite of [GingkoApp.com](https://gingkoapp.com). The latest version is available online at [gingkowriter.com](https://gingkowriter.com).

This repo contains both halves of the web app:

- `client/` — Elm + JS frontend (bundled with webpack, watched with elm-watch)
- `server/` — TypeScript/Express backend
- `data/` — local database files (created on first run, gitignored)

## Contributions Welcome!

To help **translate Gingko Writer**, join [the translation project](https://poeditor.com/join/project/k8Br3k0JVz).

For code contributions, see [client/CONTRIBUTING.md](./client/CONTRIBUTING.md).

---

## Installation & Dev Environment

### 1. Prerequisites

- [Node.js](https://nodejs.org)
- [Bun](https://bun.sh)
- [SQLite](https://sqlite.org)
- [Redis](https://redis.io) — for server-side sessions
- [CouchDB](https://couchdb.apache.org) — note your admin username and password for step 3 *

\* _This dependency will be removed once all user documents are migrated to SQLite._

Installation of these varies by system, so it's not covered here.

### 2. Clone

```
git clone git@github.com:gingko/client.git gingko
cd gingko
```

### 3. Server

```
cd server
npm i
cp config-example.js config.js
sed -i 's/couchusername/your_couchdb_admin_username/' config.js
sed -i 's/couchpassword/your_couchdb_admin_password/' config.js
npm run build
npm start
```

### 4. Client

In a new terminal:

```
cd client
bun i
cp config-example.js config.js
bun run newwatch
```

Now visit http://localhost:3000 to use your local Gingko Writer install.

## Tests

- Client end-to-end (Playwright): `cd client && bun run test`
- Server unit tests (Jest): `cd server && npm test`

---

## Desktop App (Tauri)

This checkout carries a reimplementation of the desktop app using [Tauri 2](https://tauri.app)
(replacing the Electron implementation in `client/src/electron/`), see [OVERLAY.md](./OVERLAY.md).
It reuses the same Elm apps (`client/src/elm/Electron/`) and works on local `.gkw`/`.gko` files,
with no server required.

### Prerequisites

- [Rust](https://rustup.rs) (stable toolchain)
- [Elm 0.19.1](https://guide.elm-lang.org/install/elm.html) on your PATH (or set `ELM_BINARY`)
- [Bun](https://bun.sh)
- Platform WebView dependencies, see [Tauri prerequisites](https://tauri.app/start/prerequisites/)
- `cp config-example.js config.js` in `client/` (only used for the support-contact form)

### Development

```
cd client
bun i
bun run tauri:dev
```

### Build installers

```
cd client
bun run tauri:build
```

Structure (all under `client/`):

- `src-tauri/` — Rust backend: window/menu management, file I/O with swap files and
  temp backups, per-document undo history (JSON stores under the app data dir),
  recent-documents list, txt/json export, docx export via pandoc.
- `src/tauri/` — JS glue between the Elm apps and the Tauri backend
  (port of `src/electron/renderer.js`, `home.js`, and the modals).
- `esbuild-tauri.mjs` — builds Elm + JS + static assets into `tauri-web/`,
  which Tauri serves as its frontend.
