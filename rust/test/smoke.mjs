// Runtime smoke test for the wasm-pack `--target web` output in `rust/pkg`.
//
// Run after `wasm-pack build --target web --out-dir pkg`:
//   node test/smoke.mjs
//
// This is a plain Node script rather than a `cargo test`/`wasm-bindgen-test`
// because it exists to catch integration bugs in the *published* JS/wasm glue
// (module loading, typed-array round-tripping across the wasm boundary), not
// to re-test the tree-checking logic itself, which is covered by `cargo test`.

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

// root(1,6) -> child-a(2,3), child-b(4,5)
const validTree = () => ({
	id: ['root', 'child-a', 'child-b'],
	title: ['root', 'child-a', 'child-b'],
	parent_id: [null, 'root', 'root'],
	lft: Int32Array.from([1, 2, 4]),
	rgt: Int32Array.from([6, 3, 5]),
});

const indexOf = (tree, id) => tree.id.indexOf(id);

// A valid tree round-trips with no errors and correct prefixes.
{
	const result = prepareAlbums(validTree());
	assert.equal(result.isValid, true);
	assert.equal(result.errors.length, 0);
	assert.equal(result.albums.prefix[0], '');
	assert.equal(result.albums.prefix[1], '  │ ');
	assert.equal(result.albums.trimmedId[0], 'root');
}

// Duplicate lft is detected and classified.
{
	const tree = validTree();
	tree.lft[2] = 2; // collides with child-a's lft
	const result = prepareAlbums(tree);
	assert.equal(result.isValid, false);
	assert.ok(result.errors.some((e) => e.kind === 'duplicate_left' || e.kind === 'duplicate_right'));
}

// 0 lft or rgt is detected (the "missing" sentinel).
{
	const tree = validTree();
	tree.lft[1] = 0;
	tree.rgt[2] = 0;
	const result = prepareAlbums(tree);
	const kinds = result.errors.map((e) => e.kind);
	assert.ok(kinds.includes('invalid_left'));
	assert.ok(kinds.includes('invalid_right'));
}

// An unexpected parent_id is detected.
{
	const tree = validTree();
	tree.parent_id[1] = 'someone-else';
	const result = prepareAlbums(tree);
	assert.ok(result.errors.some((e) => e.kind === 'parent'));
}

// incrementLft shifts only rows whose lft is >= the target's lft; root's
// lft (1) is below the threshold (4), so it's untouched even though its
// rgt (6) is not.
{
	const { albums } = prepareAlbums(validTree());
	const shifted = incrementLft(albums, 'child-b');
	assert.equal(shifted.lft[indexOf(shifted, 'root')], 1);
	assert.equal(shifted.rgt[indexOf(shifted, 'root')], 6);
	assert.equal(shifted.lft[indexOf(shifted, 'child-b')], 5);
	assert.equal(shifted.rgt[indexOf(shifted, 'child-b')], 6);
}

// decrementLft is the inverse of incrementLft.
{
	const { albums } = prepareAlbums(validTree());
	const shifted = incrementLft(albums, 'child-b');
	const restored = decrementLft(shifted, 'child-b');
	assert.equal(restored.lft[indexOf(restored, 'root')], 1);
	assert.equal(restored.rgt[indexOf(restored, 'root')], 6);
	assert.equal(restored.lft[indexOf(restored, 'child-b')], 4);
	assert.equal(restored.rgt[indexOf(restored, 'child-b')], 5);
}

// incrementRgt / decrementRgt round-trip through the boundary case.
{
	const { albums } = prepareAlbums(validTree());
	const widened = incrementRgt(albums, 'child-a');
	assert.equal(widened.lft[indexOf(widened, 'child-a')], 2);
	assert.equal(widened.rgt[indexOf(widened, 'child-a')], 4);
	assert.equal(widened.rgt[indexOf(widened, 'root')], 7);
}

// An id that doesn't exist leaves the arrays untouched rather than throwing.
{
	const { albums } = prepareAlbums(validTree());
	assert.doesNotThrow(() => incrementLft(albums, 'does-not-exist'));
	const untouched = incrementLft(albums, 'does-not-exist');
	assert.deepEqual(Array.from(untouched.lft), Array.from(albums.lft));
	assert.deepEqual(Array.from(untouched.rgt), Array.from(albums.rgt));
}

// getModifiedAlbums reports only rows whose lft/rgt/parent_id changed, plus
// any row that's new.
{
	const original = validTree();
	const current = validTree();
	current.lft[1] = 20;
	current.rgt[1] = 21;
	current.id = [...current.id, 'child-c'];
	current.title = [...current.title, 'child-c'];
	current.parent_id = [...current.parent_id, 'root'];
	current.lft = Int32Array.from([...current.lft, 30]);
	current.rgt = Int32Array.from([...current.rgt, 31]);

	const modified = getModifiedAlbums(current, original);
	const ids = Array.from(modified.id).sort();
	assert.deepEqual(ids, ['child-a', 'child-c']);
}

console.log('smoke test passed');
