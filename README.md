# nested-set-checker-wasm

[![CI](https://github.com/LycheeOrg/Lychee-Nested-Set-Checker-rs-ts/actions/workflows/ci.yml/badge.svg)](https://github.com/LycheeOrg/Lychee-Nested-Set-Checker-rs-ts/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/%40lychee-org%2Fnested-set-checker-wasm)](https://www.npmjs.com/package/@lychee-org/nested-set-checker-wasm)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/LycheeOrg/Lychee-Nested-Set-Checker-rs-ts/badge)](https://scorecard.dev/viewer/?uri=github.com/LycheeOrg/Lychee-Nested-Set-Checker-rs-ts)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A WebAssembly build of [Lychee](https://github.com/LycheeOrg/Lychee)'s nested-set (modified
preorder tree traversal, aka MPTT) tree checker — packaged for use from
TypeScript/JavaScript.

Lychee stores album trees as `_lft`/`_rgt` pairs. This package validates such a tree
(duplicate bounds, gaps, rows whose `parent_id` doesn't match where they sit in the
`_lft`/`_rgt` ordering) and provides the four MPTT repair operations used to fix it, all
running as compiled Wasm instead of re-walking the array in JS on every keystroke.

## Why

- **Fast on large trees.** The duplicate-detection pass and the parent-stack walk are
  both O(n); running them as Wasm keeps large album trees (thousands of rows) responsive while editing.
- **Decisions only, no rendering.** This crate returns structured results — which rows
  are duplicates, which error case applies, which rows changed — and leaves Vue
  reactivity, i18n string lookup and toast notifications to the consuming app. See
  [`examples/useTreeOperations.ts`](examples/useTreeOperations.ts) for what that
  consuming composable looks like.

## Installation

```sh
npm install @lychee-org/nested-set-checker-wasm
```

## Usage

The package is built with `wasm-pack --target web`, i.e. it's an ES module that loads
its `.wasm` file itself — no bundler plugin required, though bundlers that understand
`new URL(..., import.meta.url)` (Vite, Webpack 5, Rollup) will bundle the `.wasm` file
for you automatically.

### Browser / bundler

```ts
import init, { prepareAlbums } from "@lychee-org/nested-set-checker-wasm";

await init(); // fetches and instantiates the .wasm file once

const result = prepareAlbums([
	{ id: "root-id", title: "Root", parent_id: null, _lft: 1, _rgt: 6 },
	{ id: "child-a-id", title: "Child A", parent_id: "root-id", _lft: 2, _rgt: 3 },
	{ id: "child-b-id", title: "Child B", parent_id: "root-id", _lft: 4, _rgt: 5 },
]);

console.log(result.isValid); // true
console.log(result.albums[1].prefix); // "  │ " — indentation for display
```

`source` must already be sorted by `_lft` (same precondition as the original TS
composable).

### Node.js

`--target web` output expects to `fetch()` its own `.wasm` file, which doesn't exist in
Node. Read the file yourself and hand the bytes to `init()`:

```ts
import { readFile } from "node:fs/promises";
import init, { prepareAlbums } from "@lychee-org/nested-set-checker-wasm";

const wasmUrl = import.meta.resolve("@lychee-org/nested-set-checker-wasm/nested_set_checker_wasm_bg.wasm");
const wasmBytes = await readFile(new URL(wasmUrl));
await init({ module_or_path: wasmBytes });
```

## API

Full definitions (including every field) are shipped in the package's `.d.ts`.

```ts
function prepareAlbums(source: AlbumTree[]): PrepareResult;
function incrementLft(albums: AugmentedAlbum[], id: string): AugmentedAlbum[];
function incrementRgt(albums: AugmentedAlbum[], id: string): AugmentedAlbum[];
function decrementLft(albums: AugmentedAlbum[], id: string): AugmentedAlbum[];
function decrementRgt(albums: AugmentedAlbum[], id: string): AugmentedAlbum[];
function getModifiedAlbums(current: AlbumTree[], original: AlbumTree[]): ModifiedAlbum[];
```

| Function             | Purpose                                                                                                    |
| --------------------- | ------------------------------------------------------------------------------------------------------------ |
| `prepareAlbums`       | Validates a tree: builds duplicate `_lft`/`_rgt` sets, walks it tracking a parent stack, classifies errors. |
| `incrementLft`        | Shifts every album whose `_lft >= id`'s `_lft` up by one, making room to insert before it.                   |
| `incrementRgt`        | Shifts every album whose `_rgt >= id`'s `_rgt` up by one, making room to insert after/inside it.              |
| `decrementLft`        | Inverse of `incrementLft`.                                                                                    |
| `decrementRgt`        | Inverse of `incrementRgt`, with a safety check against collapsing a still-nonempty node.                     |
| `getModifiedAlbums`   | Diffs `current` against `original` by id, returning only the rows that actually changed (or are new).        |

`PrepareResult`:

| Field     | Type                | Description                                                        |
| --------- | ------------------- | -------------------------------------------------------------------- |
| `albums`  | `AugmentedAlbum[]`  | Input rows augmented with display prefix, trimmed ids, and per-row flags. |
| `errors`  | `ErrorDescriptor[]` | One entry per row that failed validation, with enough data to build a translated message. |
| `isValid` | `boolean`           | `errors.length === 0`.                                             |

`ErrorDescriptor.kind` is one of `"invalid_left"`, `"invalid_right"`,
`"invalid_left_right"`, `"duplicate_left"`, `"duplicate_right"`, `"parent"`, or
`"unknown"` — it maps 1:1 onto Lychee's `fix-tree.errors.<kind>` translation keys, so a
consumer only needs to look up the key and interpolate `trimmedId`/`lft`/`rgt`/`parentId`;
no error-classification logic needs to live in the frontend.

## Repository layout

```
rust/               the wasm-bindgen wrapper crate
  src/lib.rs        the entire public API surface, plus TS type declarations
  test/             a Node smoke test run against the built package in CI
examples/
  useTreeOperations.ts  a Vue composable showing how a consumer wires this
                        package into refs, i18n and toast notifications
.github/
  workflows/
    ci.yml                  tests, lints, wasm build, smoke test on every push/PR
    publish.yml             builds and publishes to npm on a GitHub Release
    dependency-review.yml   flags newly-introduced vulnerable dependencies on PRs
    scorecard.yml           OpenSSF Scorecard supply-chain analysis
  dependabot.yml    keeps GitHub Actions and Cargo dependencies up to date
  CODEOWNERS        default reviewers for pull requests
  FUNDING.yml       sponsor button configuration
```

The published npm package **is** the `wasm-pack` build output (`rust/pkg`); there's no
separate hand-written TS package to keep in sync.

## Development

Requires a Rust toolchain, the `wasm32-unknown-unknown` target, and
[`wasm-pack`](https://rustwasm.github.io/wasm-pack/):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack

cd rust
cargo test                                                    # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check

wasm-pack build --target web --out-dir pkg --release --scope lychee-org
node test/smoke.mjs                                           # exercises the built package
```

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability. Every PR is checked
by [Dependency Review](.github/workflows/dependency-review.yml) and the repo is
continuously assessed by [OpenSSF Scorecard](.github/workflows/scorecard.yml).

## Credits & license

This crate is a Rust/Wasm port of the pure tree-checking logic from Lychee's
`useTreeOperations` Vue composable. MIT licensed — see [LICENSE](LICENSE).
