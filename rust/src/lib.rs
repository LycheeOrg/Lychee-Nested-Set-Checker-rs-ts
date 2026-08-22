//! WebAssembly bindings for validating and repairing nested-set (modified
//! preorder tree traversal) album trees, as used by [Lychee](https://github.com/LycheeOrg/Lychee).
//!
//! This is a Wasm port of the pure tree/array logic from Lychee's
//! `useTreeOperations` Vue composable: duplicate `lft`/`rgt` detection, the
//! parent-stack ("pile") walk that flags rows with an unexpected `parent_id`,
//! error classification, the four MPTT repair operations, and diffing against
//! a baseline. Vue reactivity, i18n string lookup and toast notifications stay
//! in the consuming TypeScript composable — this crate only makes the
//! decisions, it doesn't render or translate anything.
//!
//! v2 note: the public API is struct-of-arrays rather than array-of-structs.
//! A tree is one object holding parallel `id`/`title`/`parent_id`/`lft`/`rgt`
//! arrays (all the same length) instead of an array of per-row objects.
//! `lft`/`rgt` cross the Wasm boundary as real `Int32Array`s and the boolean
//! per-row flags as `Uint8Array`s (0/1), avoiding a boxed JS value per cell.
//! `lft`/`rgt` are non-nullable; `0` is the "missing/invalid" sentinel
//! (mirrors how v1 already treated `null` and `0` identically).

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Sets up better panic messages in the browser/Node console.
///
/// Optional: call this once at startup. Safe to call more than once.
#[wasm_bindgen(js_name = setPanicHook)]
pub fn set_panic_hook() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// Struct-of-arrays tree: `id[i]`/`title[i]`/`parent_id[i]`/`lft[i]`/`rgt[i]`
/// together describe row `i`. All fields must have the same length.
#[derive(Debug, Clone)]
struct AlbumTree {
    id: Vec<String>,
    title: Vec<String>,
    parent_id: Vec<Option<String>>,
    lft: Vec<i32>,
    rgt: Vec<i32>,
}

/// `AlbumTree` plus the per-row fields `prepareAlbums` computes.
#[derive(Debug, Clone)]
struct AugmentedAlbumTree {
    base: AlbumTree,
    prefix: Vec<String>,
    trimmed_id: Vec<String>,
    trimmed_parent_id: Vec<String>,
    is_duplicate_rgt: Vec<bool>,
    is_duplicate_lft: Vec<bool>,
    is_expected_parent_id: Vec<bool>,
}

/// Diff output of `getModifiedAlbums`: the changed (or newly added) rows only.
struct ModifiedAlbums {
    id: Vec<String>,
    lft: Vec<i32>,
    rgt: Vec<i32>,
    parent_id: Vec<Option<String>>,
}

/// Which case of the original composable's `setErrors` if/else chain applies.
/// Serializes as `"invalid_left"`, `"invalid_right"`, etc. so a consumer can
/// build the translation key with `` `fix-tree.errors.${kind}` ``.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ErrorKind {
    InvalidLeft,
    InvalidRight,
    InvalidLeftRight,
    DuplicateLeft,
    DuplicateRight,
    Parent,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorDescriptor {
    #[serde(rename = "trimmedId")]
    trimmed_id: String,
    kind: ErrorKind,
    lft: i32,
    rgt: i32,
    #[serde(rename = "parentId")]
    parent_id: Option<String>,
}

struct PrepareResult {
    albums: AugmentedAlbumTree,
    errors: Vec<ErrorDescriptor>,
    is_valid: bool,
}

fn err_to_js(err: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&err.to_string())
}

fn trim6(s: &str) -> String {
    s.chars().take(6).collect()
}

struct PileEntry {
    parent_id: Option<String>,
    rgt: i32,
}

// --- JS <-> Rust boundary helpers -----------------------------------------
//
// The public contract (see `TS_APPEND_CONTENT` below) declares `lft`/`rgt` as
// `Int32Array` and the boolean flags as `Uint8Array` so real typed arrays
// cross the boundary instead of a JS array of boxed values. Parsing also
// accepts a plain `number[]`/`boolean[]` as a defensive fallback.

fn parse_string_vec(obj: &JsValue, key: &str) -> Result<Vec<String>, JsValue> {
    let value = js_sys::Reflect::get(obj, &JsValue::from_str(key))?;
    let array: js_sys::Array = value
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("`{key}` must be an array of strings")))?;
    array
        .iter()
        .map(|v| {
            v.as_string()
                .ok_or_else(|| JsValue::from_str(&format!("`{key}` must contain only strings")))
        })
        .collect()
}

fn parse_opt_string_vec(obj: &JsValue, key: &str) -> Result<Vec<Option<String>>, JsValue> {
    let value = js_sys::Reflect::get(obj, &JsValue::from_str(key))?;
    let array: js_sys::Array = value
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("`{key}` must be an array of strings or null")))?;
    Ok(array.iter().map(|v| v.as_string()).collect())
}

fn parse_i32_vec(obj: &JsValue, key: &str) -> Result<Vec<i32>, JsValue> {
    let value = js_sys::Reflect::get(obj, &JsValue::from_str(key))?;
    if let Some(typed) = value.dyn_ref::<js_sys::Int32Array>() {
        return Ok(typed.to_vec());
    }
    let array: js_sys::Array = value
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("`{key}` must be an Int32Array or number[]")))?;
    array
        .iter()
        .map(|v| {
            v.as_f64()
                .map(|f| f as i32)
                .ok_or_else(|| JsValue::from_str(&format!("`{key}` must contain only numbers")))
        })
        .collect()
}

fn parse_bool_vec(obj: &JsValue, key: &str) -> Result<Vec<bool>, JsValue> {
    let value = js_sys::Reflect::get(obj, &JsValue::from_str(key))?;
    if let Some(typed) = value.dyn_ref::<js_sys::Uint8Array>() {
        return Ok(typed.to_vec().into_iter().map(|b| b != 0).collect());
    }
    let array: js_sys::Array = value
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("`{key}` must be a Uint8Array or boolean[]")))?;
    Ok(array
        .iter()
        .map(|v| {
            v.as_bool()
                .unwrap_or_else(|| v.as_f64().map(|f| f != 0.0).unwrap_or(false))
        })
        .collect())
}

fn to_string_array(values: &[String]) -> JsValue {
    values
        .iter()
        .map(|s| JsValue::from_str(s))
        .collect::<js_sys::Array>()
        .into()
}

fn to_opt_string_array(values: &[Option<String>]) -> JsValue {
    values
        .iter()
        .map(|v| v.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL))
        .collect::<js_sys::Array>()
        .into()
}

fn to_i32_array(values: &[i32]) -> JsValue {
    js_sys::Int32Array::from(values).into()
}

fn to_bool_array(values: &[bool]) -> JsValue {
    let bytes: Vec<u8> = values.iter().map(|&b| b as u8).collect();
    js_sys::Uint8Array::from(bytes.as_slice()).into()
}

fn set_prop(obj: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), JsValue> {
    js_sys::Reflect::set(obj.as_ref(), &JsValue::from_str(key), value)?;
    Ok(())
}

fn album_tree_from_js(source: &JsValue) -> Result<AlbumTree, JsValue> {
    let id = parse_string_vec(source, "id")?;
    let title = parse_string_vec(source, "title")?;
    let parent_id = parse_opt_string_vec(source, "parent_id")?;
    let lft = parse_i32_vec(source, "lft")?;
    let rgt = parse_i32_vec(source, "rgt")?;

    let len = id.len();
    if title.len() != len || parent_id.len() != len || lft.len() != len || rgt.len() != len {
        return Err(JsValue::from_str(
            "AlbumTree arrays must all have the same length",
        ));
    }

    Ok(AlbumTree {
        id,
        title,
        parent_id,
        lft,
        rgt,
    })
}

fn augmented_from_js(source: &JsValue) -> Result<AugmentedAlbumTree, JsValue> {
    let base = album_tree_from_js(source)?;
    let len = base.id.len();

    let prefix = parse_string_vec(source, "prefix")?;
    let trimmed_id = parse_string_vec(source, "trimmedId")?;
    let trimmed_parent_id = parse_string_vec(source, "trimmedParentId")?;
    let is_duplicate_rgt = parse_bool_vec(source, "isDuplicate_rgt")?;
    let is_duplicate_lft = parse_bool_vec(source, "isDuplicate_lft")?;
    let is_expected_parent_id = parse_bool_vec(source, "isExpectedParentId")?;

    if prefix.len() != len
        || trimmed_id.len() != len
        || trimmed_parent_id.len() != len
        || is_duplicate_rgt.len() != len
        || is_duplicate_lft.len() != len
        || is_expected_parent_id.len() != len
    {
        return Err(JsValue::from_str(
            "AugmentedAlbumTree arrays must all have the same length",
        ));
    }

    Ok(AugmentedAlbumTree {
        base,
        prefix,
        trimmed_id,
        trimmed_parent_id,
        is_duplicate_rgt,
        is_duplicate_lft,
        is_expected_parent_id,
    })
}

fn album_tree_object(tree: &AlbumTree) -> Result<js_sys::Object, JsValue> {
    let obj = js_sys::Object::new();
    set_prop(&obj, "id", &to_string_array(&tree.id))?;
    set_prop(&obj, "title", &to_string_array(&tree.title))?;
    set_prop(&obj, "parent_id", &to_opt_string_array(&tree.parent_id))?;
    set_prop(&obj, "lft", &to_i32_array(&tree.lft))?;
    set_prop(&obj, "rgt", &to_i32_array(&tree.rgt))?;
    Ok(obj)
}

fn augmented_object(a: &AugmentedAlbumTree) -> Result<JsValue, JsValue> {
    let obj = album_tree_object(&a.base)?;
    set_prop(&obj, "prefix", &to_string_array(&a.prefix))?;
    set_prop(&obj, "trimmedId", &to_string_array(&a.trimmed_id))?;
    set_prop(
        &obj,
        "trimmedParentId",
        &to_string_array(&a.trimmed_parent_id),
    )?;
    set_prop(&obj, "isDuplicate_rgt", &to_bool_array(&a.is_duplicate_rgt))?;
    set_prop(&obj, "isDuplicate_lft", &to_bool_array(&a.is_duplicate_lft))?;
    set_prop(
        &obj,
        "isExpectedParentId",
        &to_bool_array(&a.is_expected_parent_id),
    )?;
    Ok(obj.into())
}

fn prepare_result_to_js(result: &PrepareResult) -> Result<JsValue, JsValue> {
    let obj = js_sys::Object::new();
    set_prop(&obj, "albums", &augmented_object(&result.albums)?)?;
    set_prop(
        &obj,
        "errors",
        &serde_wasm_bindgen::to_value(&result.errors).map_err(err_to_js)?,
    )?;
    set_prop(&obj, "isValid", &JsValue::from_bool(result.is_valid))?;
    Ok(obj.into())
}

fn modified_albums_to_js(m: &ModifiedAlbums) -> Result<JsValue, JsValue> {
    let obj = js_sys::Object::new();
    set_prop(&obj, "id", &to_string_array(&m.id))?;
    set_prop(&obj, "lft", &to_i32_array(&m.lft))?;
    set_prop(&obj, "rgt", &to_i32_array(&m.rgt))?;
    set_prop(&obj, "parent_id", &to_opt_string_array(&m.parent_id))?;
    Ok(obj.into())
}

// --- Pure tree/array logic (no JS types below this point) -----------------

/// A value is a duplicate if it appears more than once as either `lft` or
/// `rgt` across all rows (mirrors `buildDuplicateSets` in the TS source).
fn build_duplicate_sets(lft: &[i32], rgt: &[i32]) -> (HashSet<i32>, HashSet<i32>) {
    let mut lft_counts: HashMap<i32, u32> = HashMap::new();
    let mut rgt_counts: HashMap<i32, u32> = HashMap::new();

    for i in 0..lft.len() {
        *lft_counts.entry(lft[i]).or_insert(0) += 1;
        *rgt_counts.entry(rgt[i]).or_insert(0) += 1;
    }

    let mut duplicate_lfts = HashSet::new();
    let mut duplicate_rgts = HashSet::new();

    for i in 0..lft.len() {
        let lft_total = lft_counts.get(&lft[i]).copied().unwrap_or(0)
            + rgt_counts.get(&lft[i]).copied().unwrap_or(0);
        if lft_total > 1 {
            duplicate_lfts.insert(lft[i]);
        }

        let rgt_total = lft_counts.get(&rgt[i]).copied().unwrap_or(0)
            + rgt_counts.get(&rgt[i]).copied().unwrap_or(0);
        if rgt_total > 1 {
            duplicate_rgts.insert(rgt[i]);
        }
    }

    (duplicate_lfts, duplicate_rgts)
}

fn classify_error(
    lft: i32,
    rgt: i32,
    is_duplicate_lft: bool,
    is_duplicate_rgt: bool,
    is_expected_parent_id: bool,
) -> ErrorKind {
    if lft == 0 {
        ErrorKind::InvalidLeft
    } else if rgt == 0 {
        ErrorKind::InvalidRight
    } else if lft >= rgt {
        ErrorKind::InvalidLeftRight
    } else if is_duplicate_lft {
        ErrorKind::DuplicateLeft
    } else if is_duplicate_rgt {
        ErrorKind::DuplicateRight
    } else if !is_expected_parent_id {
        ErrorKind::Parent
    } else {
        ErrorKind::Unknown
    }
}

fn prepare_albums_impl(tree: AlbumTree) -> PrepareResult {
    let len = tree.id.len();
    let (duplicate_lfts, duplicate_rgts) = build_duplicate_sets(&tree.lft, &tree.rgt);

    let mut prefix = Vec::with_capacity(len);
    let mut trimmed_id = Vec::with_capacity(len);
    let mut trimmed_parent_id = Vec::with_capacity(len);
    let mut is_duplicate_lft = Vec::with_capacity(len);
    let mut is_duplicate_rgt = Vec::with_capacity(len);
    let mut is_expected_parent_id = Vec::with_capacity(len);
    let mut errors = Vec::new();

    let mut pile: Vec<PileEntry> = Vec::new();

    for i in 0..len {
        let row_trimmed_id = trim6(&tree.id[i]);
        let row_trimmed_parent_id = trim6(tree.parent_id[i].as_deref().unwrap_or("root"));
        let row_is_duplicate_lft = duplicate_lfts.contains(&tree.lft[i]);
        let row_is_duplicate_rgt = duplicate_rgts.contains(&tree.rgt[i]);

        // If current lft/rgt is greater than the last pile entry's rgt, we're
        // no longer inside it: pop until we're back inside the enclosing row.
        while let Some(top) = pile.last() {
            if tree.lft[i] > top.rgt || tree.rgt[i] > top.rgt {
                pile.pop();
            } else {
                break;
            }
        }

        let row_is_expected_parent_id = match pile.last() {
            Some(top) => top.parent_id == tree.parent_id[i],
            None => tree.parent_id[i].is_none(),
        };

        prefix.push("  │ ".repeat(pile.len()));

        let is_parent = tree.rgt[i] > tree.lft[i] + 1;
        if is_parent {
            pile.push(PileEntry {
                parent_id: Some(tree.id[i].clone()),
                rgt: tree.rgt[i],
            });
        }

        if tree.lft[i] == 0
            || tree.rgt[i] == 0
            || row_is_duplicate_lft
            || row_is_duplicate_rgt
            || !row_is_expected_parent_id
        {
            errors.push(ErrorDescriptor {
                trimmed_id: row_trimmed_id.clone(),
                kind: classify_error(
                    tree.lft[i],
                    tree.rgt[i],
                    row_is_duplicate_lft,
                    row_is_duplicate_rgt,
                    row_is_expected_parent_id,
                ),
                lft: tree.lft[i],
                rgt: tree.rgt[i],
                parent_id: tree.parent_id[i].clone(),
            });
        }

        trimmed_id.push(row_trimmed_id);
        trimmed_parent_id.push(row_trimmed_parent_id);
        is_duplicate_lft.push(row_is_duplicate_lft);
        is_duplicate_rgt.push(row_is_duplicate_rgt);
        is_expected_parent_id.push(row_is_expected_parent_id);
    }

    let is_valid = errors.is_empty();

    PrepareResult {
        albums: AugmentedAlbumTree {
            base: tree,
            prefix,
            trimmed_id,
            trimmed_parent_id,
            is_duplicate_rgt,
            is_duplicate_lft,
            is_expected_parent_id,
        },
        errors,
        is_valid,
    }
}

// We increment all the rows' (>= lft) left and right by 1.
fn increment_lft_impl(mut albums: AugmentedAlbumTree, id: &str) -> AugmentedAlbumTree {
    let Some(idx) = albums.base.id.iter().position(|i| i == id) else {
        return albums;
    };
    let lft = albums.base.lft[idx];

    for i in 0..albums.base.id.len() {
        if albums.base.lft[i] < lft {
            continue;
        }
        albums.base.lft[i] += 1;
        albums.base.rgt[i] += 1;
    }
    albums
}

// We increment all the rows above rgt by 1 and increment rgt by 1.
fn increment_rgt_impl(mut albums: AugmentedAlbumTree, id: &str) -> AugmentedAlbumTree {
    let Some(idx) = albums.base.id.iter().position(|i| i == id) else {
        return albums;
    };
    let rgt = albums.base.rgt[idx];

    for i in 0..albums.base.id.len() {
        let a_rgt = albums.base.rgt[i];
        if a_rgt < rgt {
            continue;
        }
        if a_rgt == rgt {
            albums.base.rgt[i] += 1;
        } else {
            albums.base.lft[i] += 1;
            albums.base.rgt[i] += 1;
        }
    }
    albums
}

// We decrement all the rows above lft by 1.
fn decrement_lft_impl(mut albums: AugmentedAlbumTree, id: &str) -> AugmentedAlbumTree {
    let Some(idx) = albums.base.id.iter().position(|i| i == id) else {
        return albums;
    };
    let lft = albums.base.lft[idx];

    for i in 0..albums.base.id.len() {
        if albums.base.lft[i] < lft {
            continue;
        }
        albums.base.lft[i] -= 1;
        albums.base.rgt[i] -= 1;
    }
    albums
}

// We decrement all the rows above rgt by 1 and decrement rgt by 1 IF lft > rgt - 1.
fn decrement_rgt_impl(mut albums: AugmentedAlbumTree, id: &str) -> AugmentedAlbumTree {
    let Some(idx) = albums.base.id.iter().position(|i| i == id) else {
        return albums;
    };
    let rgt = albums.base.rgt[idx];

    for i in 0..albums.base.id.len() {
        let a_rgt = albums.base.rgt[i];
        if a_rgt < rgt {
            continue;
        }
        // safety check
        if albums.base.lft[i] == rgt - 1 {
            continue;
        }
        if a_rgt == rgt {
            albums.base.rgt[i] -= 1;
        } else {
            // Mirrors the upstream TS composable's `decrementRgt` verbatim,
            // including its else-branch incrementing rather than
            // decrementing (matching `incrementRgt`'s else-branch). This is
            // a faithful port, not a bugfix — flagged for upstream review.
            albums.base.lft[i] += 1;
            albums.base.rgt[i] += 1;
        }
    }
    albums
}

fn get_modified_albums_impl(current: AlbumTree, original: AlbumTree) -> ModifiedAlbums {
    let mut original_index: HashMap<&str, usize> = HashMap::with_capacity(original.id.len());
    for (i, id) in original.id.iter().enumerate() {
        original_index.insert(id.as_str(), i);
    }

    let mut result = ModifiedAlbums {
        id: Vec::new(),
        lft: Vec::new(),
        rgt: Vec::new(),
        parent_id: Vec::new(),
    };

    for i in 0..current.id.len() {
        let changed = match original_index.get(current.id[i].as_str()) {
            None => true,
            Some(&oi) => {
                current.lft[i] != original.lft[oi]
                    || current.rgt[i] != original.rgt[oi]
                    || current.parent_id[i] != original.parent_id[oi]
            }
        };

        if changed {
            result.id.push(current.id[i].clone());
            result.lft.push(current.lft[i]);
            result.rgt.push(current.rgt[i]);
            result.parent_id.push(current.parent_id[i].clone());
        }
    }

    result
}

#[wasm_bindgen(typescript_custom_section)]
const TS_APPEND_CONTENT: &'static str = r#"
export interface AlbumTree {
    id: string[];
    title: string[];
    parent_id: (string | null)[];
    lft: Int32Array;
    rgt: Int32Array;
}

export interface Augmented {
    prefix: string[];
    trimmedId: string[];
    trimmedParentId: string[];
    isDuplicate_rgt: Uint8Array;
    isDuplicate_lft: Uint8Array;
    isExpectedParentId: Uint8Array;
}

export type AugmentedAlbumTree = AlbumTree & Augmented;

export type ErrorKind =
    | "invalid_left"
    | "invalid_right"
    | "invalid_left_right"
    | "duplicate_left"
    | "duplicate_right"
    | "parent"
    | "unknown";

/**
 * Everything a consumer needs to build the translated error message: the
 * `kind` maps 1:1 onto the `fix-tree.errors.<kind>` translation keys used by
 * Lychee's frontend, and `lft`/`rgt`/`parentId` are the interpolation args.
 */
export interface ErrorDescriptor {
    trimmedId: string;
    kind: ErrorKind;
    lft: number;
    rgt: number;
    parentId: string | null;
}

export interface PrepareResult {
    albums: AugmentedAlbumTree;
    errors: ErrorDescriptor[];
    isValid: boolean;
}

export interface ModifiedAlbums {
    id: string[];
    lft: Int32Array;
    rgt: Int32Array;
    parent_id: (string | null)[];
}
"#;

/// Validates a tree: builds duplicate `lft`/`rgt` sets, walks the tree in
/// `lft` order tracking a parent stack to flag rows with an unexpected
/// `parent_id`, and classifies every row that fails validation. `source`
/// should already be sorted by `lft` (the original composable relies on the
/// same precondition), and all of its arrays must have the same length.
#[wasm_bindgen(js_name = prepareAlbums, unchecked_return_type = "PrepareResult")]
pub fn prepare_albums(
    #[wasm_bindgen(unchecked_param_type = "AlbumTree")] source: JsValue,
) -> Result<JsValue, JsValue> {
    let tree = album_tree_from_js(&source)?;
    let result = prepare_albums_impl(tree);
    prepare_result_to_js(&result)
}

/// Shifts every row whose `lft` is `>= id`'s `lft` up by one, making room to
/// insert immediately before it.
#[wasm_bindgen(js_name = incrementLft, unchecked_return_type = "AugmentedAlbumTree")]
pub fn increment_lft(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbumTree")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums = augmented_from_js(&albums)?;
    augmented_object(&increment_lft_impl(albums, id))
}

/// Shifts every row whose `rgt` is `>= id`'s `rgt` up by one, making room to
/// insert immediately after it (as a sibling) or as its first child.
#[wasm_bindgen(js_name = incrementRgt, unchecked_return_type = "AugmentedAlbumTree")]
pub fn increment_rgt(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbumTree")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums = augmented_from_js(&albums)?;
    augmented_object(&increment_rgt_impl(albums, id))
}

/// Inverse of [`increment_lft`]: closes the gap left by removing a row at
/// `id`'s `lft`.
#[wasm_bindgen(js_name = decrementLft, unchecked_return_type = "AugmentedAlbumTree")]
pub fn decrement_lft(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbumTree")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums = augmented_from_js(&albums)?;
    augmented_object(&decrement_lft_impl(albums, id))
}

/// Inverse of [`increment_rgt`]: closes the gap left by removing a row at
/// `id`'s `rgt`, unless the safety check (`lft === rgt - 1`) trips.
#[wasm_bindgen(js_name = decrementRgt, unchecked_return_type = "AugmentedAlbumTree")]
pub fn decrement_rgt(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbumTree")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums = augmented_from_js(&albums)?;
    augmented_object(&decrement_rgt_impl(albums, id))
}

/// Diffs `current` against `original` by id, returning only the rows whose
/// `lft`, `rgt` or `parent_id` changed (plus any row in `current` that isn't
/// in `original` at all, i.e. newly added).
#[wasm_bindgen(js_name = getModifiedAlbums, unchecked_return_type = "ModifiedAlbums")]
pub fn get_modified_albums(
    #[wasm_bindgen(unchecked_param_type = "AlbumTree")] current: JsValue,
    #[wasm_bindgen(unchecked_param_type = "AlbumTree")] original: JsValue,
) -> Result<JsValue, JsValue> {
    let current = album_tree_from_js(&current)?;
    let original = album_tree_from_js(&original)?;
    modified_albums_to_js(&get_modified_albums_impl(current, original))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_tree() -> AlbumTree {
        // root(1,6) -> child-a(2,3), child-b(4,5)
        AlbumTree {
            id: vec!["root".into(), "child-a".into(), "child-b".into()],
            title: vec!["root".into(), "child-a".into(), "child-b".into()],
            parent_id: vec![None, Some("root".into()), Some("root".into())],
            lft: vec![1, 2, 4],
            rgt: vec![6, 3, 5],
        }
    }

    fn index_of(tree: &AugmentedAlbumTree, id: &str) -> usize {
        tree.base.id.iter().position(|i| i == id).unwrap()
    }

    #[test]
    fn valid_tree_has_no_errors() {
        let result = prepare_albums_impl(valid_tree());
        assert!(result.is_valid);
        assert!(result.errors.is_empty());
        assert_eq!(result.albums.prefix[0], "");
        assert_eq!(result.albums.prefix[1], "  │ ");
    }

    #[test]
    fn detects_duplicate_lft_and_rgt() {
        let mut tree = valid_tree();
        tree.lft[2] = 2; // now collides with child-a's lft
        let result = prepare_albums_impl(tree);
        assert!(!result.is_valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.kind == ErrorKind::DuplicateLeft || e.kind == ErrorKind::DuplicateRight));
    }

    #[test]
    fn detects_zero_lft_rgt() {
        let mut tree = valid_tree();
        tree.lft[1] = 0;
        tree.rgt[2] = 0;
        let result = prepare_albums_impl(tree);
        assert!(!result.is_valid);
        let kinds: Vec<_> = result.errors.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&ErrorKind::InvalidLeft));
        assert!(kinds.contains(&ErrorKind::InvalidRight));
    }

    #[test]
    fn detects_lft_gte_rgt() {
        let mut tree = valid_tree();
        tree.lft[1] = 3;
        tree.rgt[1] = 3;
        let result = prepare_albums_impl(tree);
        assert!(result
            .errors
            .iter()
            .any(|e| e.kind == ErrorKind::InvalidLeftRight));
    }

    #[test]
    fn detects_unexpected_parent_id() {
        let mut tree = valid_tree();
        tree.parent_id[1] = Some("someone-else".to_string());
        let result = prepare_albums_impl(tree);
        assert!(result.errors.iter().any(|e| e.kind == ErrorKind::Parent));
    }

    #[test]
    fn increment_lft_shifts_everything_at_or_after() {
        let result = prepare_albums_impl(valid_tree());
        let shifted = increment_lft_impl(result.albums, "child-b");
        // Only rows whose lft >= 4 move; root's lft is 1, so it (and its
        // rgt) is left untouched even though 6 >= 4.
        assert_eq!(shifted.base.lft[index_of(&shifted, "root")], 1);
        assert_eq!(shifted.base.rgt[index_of(&shifted, "root")], 6);
        assert_eq!(shifted.base.lft[index_of(&shifted, "child-a")], 2);
        assert_eq!(shifted.base.lft[index_of(&shifted, "child-b")], 5);
        assert_eq!(shifted.base.rgt[index_of(&shifted, "child-b")], 6);
    }

    #[test]
    fn increment_rgt_widens_the_target_only_at_boundary() {
        let result = prepare_albums_impl(valid_tree());
        let shifted = increment_rgt_impl(result.albums, "child-a");
        assert_eq!(shifted.base.rgt[index_of(&shifted, "child-a")], 4);
        assert_eq!(shifted.base.lft[index_of(&shifted, "child-a")], 2); // exact rgt match: only rgt moves
        assert_eq!(shifted.base.lft[index_of(&shifted, "child-b")], 5);
        assert_eq!(shifted.base.rgt[index_of(&shifted, "child-b")], 6);
        assert_eq!(shifted.base.rgt[index_of(&shifted, "root")], 7);
    }

    #[test]
    fn decrement_is_inverse_of_increment_for_lft() {
        let result = prepare_albums_impl(valid_tree());
        let shifted = increment_lft_impl(result.albums, "child-b");
        let restored = decrement_lft_impl(shifted, "child-b");
        assert_eq!(restored.base.lft[index_of(&restored, "root")], 1);
        assert_eq!(restored.base.rgt[index_of(&restored, "root")], 6);
        assert_eq!(restored.base.lft[index_of(&restored, "child-b")], 4);
        assert_eq!(restored.base.rgt[index_of(&restored, "child-b")], 5);
    }

    #[test]
    fn unknown_id_leaves_albums_untouched() {
        let result = prepare_albums_impl(valid_tree());
        let before = result.albums.clone();
        let untouched = increment_lft_impl(result.albums, "does-not-exist");
        assert_eq!(untouched.base.lft, before.base.lft);
        assert_eq!(untouched.base.rgt, before.base.rgt);
    }

    #[test]
    fn get_modified_albums_reports_only_changes() {
        let original = valid_tree();
        let mut current = valid_tree();
        current.lft[1] = 20;
        current.rgt[1] = 21;
        current.id.push("child-c".into());
        current.title.push("child-c".into());
        current.parent_id.push(Some("root".into()));
        current.lft.push(30);
        current.rgt.push(31);

        let modified = get_modified_albums_impl(current, original);
        let ids: HashSet<_> = modified.id.into_iter().collect();
        assert_eq!(
            ids,
            HashSet::from(["child-a".to_string(), "child-c".to_string()])
        );
    }
}
