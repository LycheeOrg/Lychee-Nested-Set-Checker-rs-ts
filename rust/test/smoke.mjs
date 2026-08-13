// Runtime smoke test for the wasm-pack `--target web` output in `rust/pkg`.
//
// Run after `wasm-pack build --target web --out-dir pkg`:
//   node test/smoke.mjs
//
// This is a plain Node script rather than a `cargo test`/`wasm-bindgen-test`
// because it exists to catch integration bugs in the *published* JS/wasm glue
// (module loading, JSON round-tripping across the wasm boundary), not to
// re-test the tree-checking logic itself, which is covered by `cargo test`.

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

import init, {
	prepareAlbums,
	incrementLft,
	incrementRgt,
	decrementLft,
	decrementRgt,
	getModifiedAlbums,
	setPanicHook,
} from '../pkg/nested_set_checker_wasm.js';

const wasmBytes = await readFile(new URL('../pkg/nested_set_checker_wasm_bg.wasm', import.meta.url));
await init({ module_or_path: wasmBytes });
setPanicHook();

const album = (id, parent_id, _lft, _rgt) => ({ id, title: id, parent_id, _lft, _rgt });

const validTree = () => [
	album('root', null, 1, 6),
	album('child-a', 'root', 2, 3),
	album('child-b', 'root', 4, 5),
];

// A valid tree round-trips with no errors and correct prefixes.
{
	const result = prepareAlbums(validTree());
	assert.equal(result.isValid, true);
	assert.equal(result.errors.length, 0);
	assert.equal(result.albums[0].prefix, '');
	assert.equal(result.albums[1].prefix, '  │ ');
	assert.equal(result.albums[0].trimmedId, 'root');
}

// Duplicate _lft is detected and classified.
{
	const tree = validTree();
	tree[2]._lft = 2; // collides with child-a's _lft
	const result = prepareAlbums(tree);
	assert.equal(result.isValid, false);
	assert.ok(result.errors.some((e) => e.kind === 'duplicate_left' || e.kind === 'duplicate_right'));
}

// null/0 _lft or _rgt is detected.
{
	const tree = validTree();
	tree[1]._lft = null;
	tree[2]._rgt = 0;
	const result = prepareAlbums(tree);
	const kinds = result.errors.map((e) => e.kind);
	assert.ok(kinds.includes('invalid_left'));
	assert.ok(kinds.includes('invalid_right'));
}

// An unexpected parent_id is detected.
{
	const tree = validTree();
	tree[1].parent_id = 'someone-else';
	const result = prepareAlbums(tree);
	assert.ok(result.errors.some((e) => e.kind === 'parent'));
}

// incrementLft shifts only rows whose _lft is >= the target's _lft; root's
// _lft (1) is below the threshold (4), so it's untouched even though its
// _rgt (6) is not.
{
	const { albums } = prepareAlbums(validTree());
	const shifted = incrementLft(albums, 'child-b');
	const byId = Object.fromEntries(shifted.map((a) => [a.id, a]));
	assert.equal(byId.root._lft, 1);
	assert.equal(byId.root._rgt, 6);
	assert.equal(byId['child-b']._lft, 5);
	assert.equal(byId['child-b']._rgt, 6);
}

// decrementLft is the inverse of incrementLft.
{
	const { albums } = prepareAlbums(validTree());
	const shifted = incrementLft(albums, 'child-b');
	const restored = decrementLft(shifted, 'child-b');
	const byId = Object.fromEntries(restored.map((a) => [a.id, a]));
	assert.equal(byId.root._lft, 1);
	assert.equal(byId.root._rgt, 6);
	assert.equal(byId['child-b']._lft, 4);
	assert.equal(byId['child-b']._rgt, 5);
}

// incrementRgt / decrementRgt round-trip through the boundary case.
{
	const { albums } = prepareAlbums(validTree());
	const widened = incrementRgt(albums, 'child-a');
	const byId = Object.fromEntries(widened.map((a) => [a.id, a]));
	assert.equal(byId['child-a']._lft, 2);
	assert.equal(byId['child-a']._rgt, 4);
	assert.equal(byId.root._rgt, 7);
}

// An id that doesn't exist leaves the array untouched rather than throwing.
{
	const { albums } = prepareAlbums(validTree());
	assert.doesNotThrow(() => incrementLft(albums, 'does-not-exist'));
	const untouched = incrementLft(albums, 'does-not-exist');
	assert.deepEqual(
		untouched.map((a) => [a._lft, a._rgt]),
		albums.map((a) => [a._lft, a._rgt]),
	);
}

// getModifiedAlbums reports only rows whose _lft/_rgt/parent_id changed, plus
// any row that's new.
{
	const original = validTree();
	const current = validTree();
	current[1]._lft = 20;
	current[1]._rgt = 21;
	current.push(album('child-c', 'root', 30, 31));

	const modified = getModifiedAlbums(current, original);
	const ids = modified.map((m) => m.id).sort();
	assert.deepEqual(ids, ['child-a', 'child-c']);
}

console.log('smoke test passed');
