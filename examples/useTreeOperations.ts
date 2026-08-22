// Example: how a Lychee frontend composable wires `@lychee-org/nested-set-checker-wasm`
// into Vue refs, i18n and toast notifications. This mirrors the original
// `useTreeOperations` composable's public API (see the project this package was ported
// from) but delegates every pure tree computation — duplicate detection, the
// parent-stack walk, error classification, the four MPTT repair ops, and diffing — to
// the Wasm module. Only string localization and Vue reactivity live here.
//
// v2: albums are struct-of-arrays, not an array of per-row objects. `_lft`/`_rgt` cross
// the Wasm boundary as `Int32Array`s and per-row boolean flags as `Uint8Array`s (0/1)
// instead of a JS array of boxed values per row.

import { trans } from "laravel-vue-i18n";
import { type ToastLike } from "@/composables/toast-contract";
import { sprintf } from "sprintf-js";
import { ref, type Ref } from "vue";
import init, {
	prepareAlbums as wasmPrepareAlbums,
	incrementLft as wasmIncrementLft,
	incrementRgt as wasmIncrementRgt,
	decrementLft as wasmDecrementLft,
	decrementRgt as wasmDecrementRgt,
	getModifiedAlbums as wasmGetModifiedAlbums,
	setPanicHook,
	type AlbumTree as WasmAlbumTree,
	type AugmentedAlbumTree as WasmAugmentedAlbumTree,
	type ErrorDescriptor,
} from "@lychee-org/nested-set-checker-wasm";

export type Augmented = {
	prefix: string[];
	trimmedId: string[];
	trimmedParentId: string[];
	isDuplicate_rgt: Uint8Array;
	isDuplicate_lft: Uint8Array;
	isExpectedParentId: Uint8Array;
};

export type AlbumTree = {
	id: string[];
	title: string[];
	parent_id: (string | null)[];
	_lft: Int32Array;
	_rgt: Int32Array;
};

export type AugmentedAlbumTree = AlbumTree & Augmented;

let wasmReady: Promise<void> | null = null;

// The Wasm module must be `init()`ed exactly once before any of its exports can be
// called. `prepareAlbums`/`check` await this; every other operation below assumes it has
// already resolved, which holds as long as callers always populate `albums` via
// `prepareAlbums` first (the same precondition the original composable had for `albums`
// being defined at all).
function ensureWasm(): Promise<void> {
	if (wasmReady === null) {
		wasmReady = init().then(() => setPanicHook());
	}
	return wasmReady;
}

const ERROR_TRANS_ARGS: Record<ErrorDescriptor["kind"], (e: ErrorDescriptor) => unknown[]> = {
	invalid_left: (e) => [e.trimmedId],
	invalid_right: (e) => [e.trimmedId],
	invalid_left_right: (e) => [e.trimmedId, e.lft, e.rgt],
	duplicate_left: (e) => [e.trimmedId, e.lft],
	duplicate_right: (e) => [e.trimmedId, e.rgt],
	parent: (e) => [e.trimmedId, e.parentId ?? "root"],
	unknown: (e) => [e.trimmedId],
};

// `ErrorDescriptor.kind` maps 1:1 onto the `fix-tree.errors.<kind>` translation keys;
// only the string lookup and interpolation happen here, not the decision of which error
// applies to a given row (that's `prepareAlbums`, in Rust).
function formatError(e: ErrorDescriptor): string {
	return sprintf(trans(`fix-tree.errors.${e.kind}`), ...ERROR_TRANS_ARGS[e.kind](e));
}

// Sorts a struct-of-arrays tree by `_lft`, keeping every field's columns aligned.
function sortByLft<T extends AlbumTree>(tree: T): T {
	const order = Array.from(tree.id, (_, i) => i).sort((a, b) => tree._lft[a] - tree._lft[b]);
	const pick = <U>(values: U[] | Int32Array | Uint8Array): U[] | Int32Array | Uint8Array => {
		if (values instanceof Int32Array) return Int32Array.from(order, (i) => values[i]);
		if (values instanceof Uint8Array) return Uint8Array.from(order, (i) => values[i]);
		return order.map((i) => values[i]);
	};
	const sorted = { ...tree };
	for (const key of Object.keys(sorted) as (keyof T)[]) {
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		sorted[key] = pick(sorted[key] as any) as any;
	}
	return sorted;
}

export function useTreeOperations(
	originalAlbums: Ref<AlbumTree | undefined>,
	albums: Ref<AugmentedAlbumTree | undefined>,
	toast: ToastLike,
) {
	const isValidated = ref(false);
	const errors = ref<string[]>([]);

	async function prepareAlbums(sourceAlbums?: AlbumTree) {
		// Use provided source, or fall back to originalAlbums for initial load
		const source = sourceAlbums ?? originalAlbums.value;
		if (source === undefined) {
			return;
		}

		await ensureWasm();
		const result = wasmPrepareAlbums(source as WasmAlbumTree);

		albums.value = result.albums as AugmentedAlbumTree;
		errors.value = result.errors.map(formatError);
		isValidated.value = result.isValid;
	}

	function validate() {
		return errors.value.length === 0;
	}

	function check() {
		if (albums.value === undefined) {
			return;
		}
		// Sort current albums and revalidate without overwriting the baseline
		const sortedAlbums = sortByLft(albums.value);
		void prepareAlbums(sortedAlbums).then(() => {
			errors.value.forEach((e) => toast.add({ severity: "error", summary: trans("toasts.error"), detail: e, life: 3000 }));
		});
	}

	function incrementLft(id: string) {
		if (albums.value === undefined) {
			return;
		}
		albums.value = wasmIncrementLft(albums.value as WasmAugmentedAlbumTree, id) as AugmentedAlbumTree;
	}

	function incrementRgt(id: string) {
		if (albums.value === undefined) {
			return;
		}
		albums.value = wasmIncrementRgt(albums.value as WasmAugmentedAlbumTree, id) as AugmentedAlbumTree;
	}

	function decrementLft(id: string) {
		if (albums.value === undefined) {
			return;
		}
		albums.value = wasmDecrementLft(albums.value as WasmAugmentedAlbumTree, id) as AugmentedAlbumTree;
	}

	function decrementRgt(id: string) {
		if (albums.value === undefined) {
			return;
		}
		albums.value = wasmDecrementRgt(albums.value as WasmAugmentedAlbumTree, id) as AugmentedAlbumTree;
	}

	function getModifiedAlbums(): { id: string[]; _lft: Int32Array; _rgt: Int32Array; parent_id: (string | null)[] } {
		if (albums.value === undefined || originalAlbums.value === undefined) {
			return { id: [], _lft: new Int32Array(), _rgt: new Int32Array(), parent_id: [] };
		}
		return wasmGetModifiedAlbums(albums.value as WasmAlbumTree, originalAlbums.value as WasmAlbumTree);
	}

	return {
		isValidated,
		validate,
		prepareAlbums,
		check,
		incrementLft,
		incrementRgt,
		decrementLft,
		decrementRgt,
		getModifiedAlbums,
	};
}
