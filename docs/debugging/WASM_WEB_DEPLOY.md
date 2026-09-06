# Deploying the web page to GitHub Pages (issue #53)

**Date:** 2026-09-06
**Tracking:** GitHub issue #53 (Phase 5 of `docs/plans/WASM_WEB.md`)

## What changed

- `.github/workflows/pages.yml`: builds `web/` with wasm-pack and
  deploys the result to GitHub Pages on every pushed tag matching `v*`
  (and on `workflow_dispatch` for a manual run). Two jobs: `build`
  (toolchain with the `wasm32-unknown-unknown` target, rust-cache, Node
  22, wasm-pack from its installer script, `npm run build`,
  `npm run check-size`, site assembly, `actions/configure-pages`,
  `actions/upload-pages-artifact`) and `deploy` (`actions/deploy-pages`
  in the `github-pages` environment). Permissions are `contents: read`,
  `pages: write`, `id-token: write`; the `pages` concurrency group
  serialises deploys without cancelling one in flight.
- `web/scripts/check-size.mjs`, wired to `npm run check-size`: the size
  budget as a script so it runs the same way locally, in CI and in the
  Pages workflow.
- `.github/workflows/ci.yml`: the wasm job's "Report wasm size" step
  (an `ls -la`) is now `npm run check-size -- pkg-node/nes_emu_web_bg.wasm`,
  so a pull request that pushes the module over budget fails CI rather
  than being noticed at tag time.
- `README.md`: a "Play in the browser" section with the hosted link, the
  note that users supply their own ROMs and nothing copyrighted is
  hosted, and the local build and serve commands (details stay in
  `web/README.md`).

## Site layout

The workflow copies only what the page needs at runtime into `site/`
and uploads that directory:

```
site/
  .nojekyll            tells Pages not to run Jekyll (cheap insurance)
  index.html
  app.js, audio-worklet.js, storage.js   every top-level web/*.js module
  pkg/nes_emu_web.js       wasm-bindgen glue
  pkg/nes_emu_web_bg.wasm  the module
  cheats.json              only if web/cheats.json exists after the build
```

`web/pkg/` also contains `.d.ts` files, a `package.json`, a `README.md`
and a `.gitignore`; they are not copied. `web/test/`, `web/scripts/`,
`node_modules`, the crate sources and the package files never reach the
site. `cheats.json` is produced by a parallel lane (issue #51); the
copy is guarded with `if [ -f ... ]` because GitHub runs `run:` steps
with `bash -e` and a trailing `[ -f x ] && cp x y` would fail the step
whenever the file is absent.

The page imports `./pkg/nes_emu_web.js` relative to `index.html`, so
it works at any base path, including the project path
`https://codyaverett.github.io/nes-emu/`. GitHub Pages serves `.wasm`
as `application/wasm`, so `WebAssembly.instantiateStreaming` is used;
wasm-bindgen's `init` falls back to `arrayBuffer` when a server sends
another type.

## Size budget

`npm run check-size [path ...]` (default: whichever of
`pkg/nes_emu_web_bg.wasm` and `pkg-node/nes_emu_web_bg.wasm` exist)
prints the module size in bytes and KB (1 KB = 1024 bytes) and the
names of its custom sections, and exits 1 when

- the module is larger than 500 KB (512 000 bytes), or
- it carries debug information: a `name` custom section or any
  `.debug_*` section.

The current module is 163 056 bytes (159.2 KB) with only the
`producers` and `target_features` custom sections, so there is no debug
info to strip: wasm-pack already runs `wasm-opt` on release builds and
the root `Cargo.toml` sets `opt-level = "s"` for the `nes-emu-web`
package. The check exists so a future profile change (`debug = true`,
`strip = false`, a dependency that balloons the module) is caught by
CI and the Pages workflow instead of by users on a slow connection.

Both workflows fail on a violation; there is no report-only mode. To
raise the budget, change `BUDGET_BYTES` in the script and this
document.

## Verified locally

- `cd web && npm run build`: 163 056 bytes (159.2 KB) wasm, custom
  sections `producers, target_features`; `npm run check-size` passes.
- The script's failure paths, with synthetic modules: a module carrying
  a `name` section and a 600 KB module both exit 1 with the reason
  printed.
- The site assembly step replicated as a shell snippet into a scratch
  directory; the result held exactly the six files listed above (no
  `cheats.json` yet, as expected on this branch).
- The assembled directory served with
  `python3 -m http.server PORT --bind 127.0.0.1 --directory site` and
  loaded in headless Chromium through Playwright: `index.html`,
  `app.js`, `pkg/nes_emu_web.js` and `pkg/nes_emu_web_bg.wasm` all
  returned 200, `window.nesStats.core` was `0.13.0`, `nesStats.error`
  null, the console held `[nes] core 0.13.0 ready` and no errors or
  warnings.
- The same directory copied under a `nes-emu/` prefix and loaded at
  `http://127.0.0.1:PORT/nes-emu/`, mirroring the project path of the
  hosted URL: the four requests came back 200 under `/nes-emu/` and
  `nesStats.core` was set, so every reference in the page is relative
  (`grep` for `src=`, `addModule` and `new URL` finds only `app.js`
  and `./audio-worklet.js`).
- `pages.yml` and `ci.yml` parse as YAML (`ruby -ryaml`).
- All four native gates and `cd web && npm test`.

## Not verified until the first tag deploy

- The Pages deployment itself. It needs the repository's Pages source
  set to "GitHub Actions" (Settings, Pages) and the `github-pages`
  environment, which the maintainer enables at merge time; the URL
  https://codyaverett.github.io/nes-emu/ is live only after the first
  `v*` tag (or a manual `workflow_dispatch` run) completes.
- wasm-pack installation and the wasm-opt step on the Ubuntu runner
  (the same install line has run in CI's wasm job since issue #49).
- The AudioWorklet over HTTPS on the real host. This session loaded no
  ROM, so audio stayed `off`; the worklet was verified on `127.0.0.1`
  (also a secure context) in issue #50.

## Debugging steps

1. `python3 -m http.server` on the first port tried reported
   `Address already in use` (another local tool was listening); moved
   to a free port. Not a site problem.
2. Playwright's `page.evaluate` in this MCP server does not accept a
   top-level `await`; the check was written as a synchronous expression
   after navigating with `waitUntil: networkidle`, by which time the
   module had already initialised.
3. The console log buffer of the Playwright server still held the
   `[nes]` pacing lines from the issue #50 session; filtering by type
   `error` and `warning` returned nothing for this page load.

## First deploy (v0.14.0, 2026-09-06)

The tag `v0.14.0` triggered the workflow. Two failures, both fixed:

1. **Deploy job rejected.** The build job passed (module 164 429 bytes,
   160.6 KB) but "Deploy to GitHub Pages" failed immediately. Enabling
   Pages with the Actions source creates a `github-pages` environment
   whose deployment branch policy allows only `main`, so a run started by
   a tag ref is not allowed to deploy. Fix (repository setting, not a
   file): a second policy `v*` of type `tag` on that environment,
   added with

   ```sh
   gh api -X POST repos/codyaverett/nes-emu/environments/github-pages/deployment-branch-policies -f name='v*' -f type=tag
   ```

   Re-running the failed job then deployed and every site file answered
   200.
2. **Page did not boot.** Headless Chromium on the hosted URL showed the
   cheat database, the wasm module (served as `application/wasm`) and
   the AudioWorklet all loading over HTTPS, but `window.nesStats` never
   appeared and one request returned 404: `storage.js`. The assembly
   step listed the page files by name (`index.html app.js
   audio-worklet.js`) and issue #51 added `storage.js` in a parallel
   lane after this workflow was written; an ES module import that 404s
   aborts the whole module graph, so `app.js` never ran. Fix: copy
   `web/*.js` (every top-level module; `scripts/` and `test/` are
   directories and do not match) and a check after assembly that every
   relative `from "./x"` import in `site/*.js` exists in the site, which
   fails the build with a `::error::` annotation otherwise.
