// Build the Tauri desktop frontend into ./tauri-web
// (Elm apps + JS bundles + static assets).
import * as esbuild from 'esbuild'
import * as fs from 'node:fs'
import * as path from 'node:path'
import { execFileSync, execSync } from 'node:child_process'
import { replaceKernelPackages } from './elm-kernel-replacements/replace-kernel-packages.mjs'

const ROOT = import.meta.dirname ?? path.dirname(new URL(import.meta.url).pathname)
const OUT = path.join(ROOT, 'tauri-web')
const isProd = process.argv.includes('--production')

fs.mkdirSync(OUT, { recursive: true })

/* ==== 1. Elm kernel patches (same as web build) ==== */
replaceKernelPackages()

/* ==== 2. Compile the desktop Elm apps into one bundle ==== */

function findElm () {
  if (process.env.ELM_BINARY) return process.env.ELM_BINARY
  const candidates = [
    'elm',
    'C:\\Program Files (x86)\\Elm\\0.19.1\\bin\\elm.exe',
    path.join(ROOT, 'node_modules', 'elm', 'bin', process.platform === 'win32' ? 'elm.exe' : 'elm')
  ]
  for (const c of candidates) {
    try {
      execFileSync(c, ['--version'], { stdio: 'pipe' })
      return c
    } catch { /* try next */ }
  }
  throw new Error('elm binary not found — install Elm 0.19.1 or set ELM_BINARY')
}

const elm = findElm()
const elmArgs = [
  'make',
  'src/elm/Electron/Electron.elm',
  'src/elm/Electron/Home.elm',
  'src/elm/Electron/Worker.elm',
  'src/elm/Electron/ShortcutsModal.elm',
  'src/elm/Electron/VideosModal.elm',
  'src/elm/Electron/SupportModal.elm',
  `--output=${path.join(OUT, 'desktop-elm.js')}`
]
if (isProd) elmArgs.push('--optimize')

execFileSync(elm, elmArgs, {
  cwd: ROOT,
  stdio: 'inherit',
  env: { ...process.env, ELM_HOME: process.env.ELM_HOME || path.join(ROOT, 'elm-home', 'elm-stuff') }
})

/* ==== 3. Bundle the JS entry points ==== */

const result = await esbuild.build({
  entryPoints: [
    './src/tauri/renderer.js',
    './src/tauri/home.js',
    './src/tauri/shortcuts.js',
    './src/tauri/videos.js',
    './src/tauri/support.js'
  ],
  outdir: OUT,
  minify: isProd,
  platform: 'browser',
  bundle: true,
  define: {
    global: 'window',
    'process.env.NODE_ENV': isProd ? '"production"' : '"development"'
  }
})

if (result.errors.length > 0) {
  console.error(result.errors)
  process.exit(1)
}

/* ==== 4. Static assets ==== */

fs.cpSync(path.join(ROOT, 'src', 'static'), OUT, { recursive: true })
fs.cpSync(path.join(ROOT, 'src', 'tauri', 'static'), OUT, { recursive: true })

/* ==== 5. Tailwind (style.css is the tailwind input) ==== */

execSync(`bunx tailwindcss -i ${path.join(ROOT, 'src', 'static', 'style.css')} -o ${path.join(OUT, 'style.css')}${isProd ? ' --minify' : ''}`, {
  cwd: ROOT,
  stdio: 'inherit'
})

console.log('\x1b[32m%s\x1b[0m', 'Tauri frontend build succeeded → tauri-web/')
