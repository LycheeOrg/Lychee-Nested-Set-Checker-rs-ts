//! WebAssembly bindings for validating and repairing nested-set (modified
//! preorder tree traversal) album trees, as used by [Lychee](https://github.com/LycheeOrg/Lychee).
//!
//! This is a Wasm port of the pure tree/array logic from Lychee's
//! `useTreeOperations` Vue composable: duplicate `_lft`/`_rgt` detection, the
//! parent-stack ("pile") walk that flags rows with an unexpected `parent_id`,
//! error classification, the four MPTT repair operations, and diffing against
//! a baseline. Vue reactivity, i18n string lookup and toast notifications stay
//! in the consuming TypeScript composable — this crate only makes the
//! decisions, it doesn't render or translate anything.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use wasm_bindgen::prelude::*;

/// Sets up better panic messages in the browser/Node console.
///
/// Optional: call this once at startup. Safe to call more than once.
#[wasm_bindgen(js_name = setPanicHook)]
pub fn set_panic_hook() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[derive(Debug, Clone, Deserialize)]
struct AlbumTree {
    id: String,
    title: String,
    parent_id: Option<String>,
    _lft: Option<i64>,
    _rgt: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AugmentedAlbum {
    id: String,
    title: String,
    parent_id: Option<String>,
    _lft: Option<i64>,
    _rgt: Option<i64>,
    prefix: String,
    #[serde(rename = "trimmedId")]
    trimmed_id: String,
    #[serde(rename = "trimmedParentId")]
    trimmed_parent_id: String,
    #[serde(rename = "isDuplicate_rgt")]
    is_duplicate_rgt: bool,
    #[serde(rename = "isDuplicate_lft")]
    is_duplicate_lft: bool,
    #[serde(rename = "isExpectedParentId")]
    is_expected_parent_id: bool,
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
    lft: Option<i64>,
    rgt: Option<i64>,
    #[serde(rename = "parentId")]
    parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PrepareResult {
    albums: Vec<AugmentedAlbum>,
    errors: Vec<ErrorDescriptor>,
    #[serde(rename = "isValid")]
    is_valid: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ModifiedAlbum {
    id: String,
    _lft: Option<i64>,
    _rgt: Option<i64>,
    parent_id: Option<String>,
}

/// The original TS types declare `_lft`/`_rgt` as non-nullable `number`, but
/// `isError` defensively checks for `null` anyway (real-world rows can be
/// mid-repair). JS arithmetic and comparisons coerce `null`/`undefined` to
/// `0`; this mirrors that coercion explicitly instead of panicking on `None`.
fn n(v: Option<i64>) -> i64 {
    v.unwrap_or(0)
}

fn err_to_js(err: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&err.to_string())
}

fn trim6(s: &str) -> String {
    s.chars().take(6).collect()
}

struct PileEntry {
    parent_id: Option<String>,
    rgt: i64,
}

/// A value is a duplicate if it appears more than once as either `_lft` or
/// `_rgt` across all albums (mirrors `buildDuplicateSets` in the TS source).
fn build_duplicate_sets(albums: &[AlbumTree]) -> (HashSet<Option<i64>>, HashSet<Option<i64>>) {
    let mut lft_counts: HashMap<Option<i64>, u32> = HashMap::new();
    let mut rgt_counts: HashMap<Option<i64>, u32> = HashMap::new();

    for album in albums {
        *lft_counts.entry(album._lft).or_insert(0) += 1;
        *rgt_counts.entry(album._rgt).or_insert(0) += 1;
    }

    let mut duplicate_lfts = HashSet::new();
    let mut duplicate_rgts = HashSet::new();

    for album in albums {
        let lft_total = lft_counts.get(&album._lft).copied().unwrap_or(0)
            + rgt_counts.get(&album._lft).copied().unwrap_or(0);
        if lft_total > 1 {
            duplicate_lfts.insert(album._lft);
        }

        let rgt_total = lft_counts.get(&album._rgt).copied().unwrap_or(0)
            + rgt_counts.get(&album._rgt).copied().unwrap_or(0);
        if rgt_total > 1 {
            duplicate_rgts.insert(album._rgt);
        }
    }

    (duplicate_lfts, duplicate_rgts)
}

fn is_error(album: &AugmentedAlbum) -> bool {
    album._lft.is_none()
        || album._rgt.is_none()
        || album._lft == Some(0)
        || album._rgt == Some(0)
        || album.is_duplicate_lft
        || album.is_duplicate_rgt
        || !album.is_expected_parent_id
}

fn classify_error(album: &AugmentedAlbum) -> ErrorKind {
    if album._lft.is_none() || album._lft == Some(0) {
        ErrorKind::InvalidLeft
    } else if album._rgt.is_none() || album._rgt == Some(0) {
        ErrorKind::InvalidRight
    } else if n(album._lft) >= n(album._rgt) {
        ErrorKind::InvalidLeftRight
    } else if album.is_duplicate_lft {
        ErrorKind::DuplicateLeft
    } else if album.is_duplicate_rgt {
        ErrorKind::DuplicateRight
    } else if !album.is_expected_parent_id {
        ErrorKind::Parent
    } else {
        ErrorKind::Unknown
    }
}

fn prepare_albums_impl(source: Vec<AlbumTree>) -> PrepareResult {
    let (duplicate_lfts, duplicate_rgts) = build_duplicate_sets(&source);

    let mut albums = Vec::with_capacity(source.len());
    let mut pile: Vec<PileEntry> = Vec::new();

    for album in source {
        let trimmed_id = trim6(&album.id);
        let trimmed_parent_id = trim6(album.parent_id.as_deref().unwrap_or("root"));
        let is_duplicate_lft = duplicate_lfts.contains(&album._lft);
        let is_duplicate_rgt = duplicate_rgts.contains(&album._rgt);

        // If current lft/rgt is greater than the last pile entry's rgt, we're
        // no longer inside it: pop until we're back inside the enclosing album.
        while let Some(top) = pile.last() {
            if n(album._lft) > top.rgt || n(album._rgt) > top.rgt {
                pile.pop();
            } else {
                break;
            }
        }

        let is_expected_parent_id = match pile.last() {
            Some(top) => top.parent_id == album.parent_id,
            None => album.parent_id.is_none(),
        };

        let prefix = "  │ ".repeat(pile.len());
        let is_parent = n(album._rgt) > n(album._lft) + 1;

        let AlbumTree {
            id,
            title,
            parent_id,
            _lft,
            _rgt,
        } = album;

        if is_parent {
            pile.push(PileEntry {
                parent_id: Some(id.clone()),
                rgt: n(_rgt),
            });
        }

        albums.push(AugmentedAlbum {
            id,
            title,
            parent_id,
            _lft,
            _rgt,
            prefix,
            trimmed_id,
            trimmed_parent_id,
            is_duplicate_lft,
            is_duplicate_rgt,
            is_expected_parent_id,
        });
    }

    let mut errors = Vec::new();
    for album in &albums {
        if is_error(album) {
            errors.push(ErrorDescriptor {
                trimmed_id: album.trimmed_id.clone(),
                kind: classify_error(album),
                lft: album._lft,
                rgt: album._rgt,
                parent_id: album.parent_id.clone(),
            });
        }
    }
    let is_valid = errors.is_empty();

    PrepareResult {
        albums,
        errors,
        is_valid,
    }
}

// We increment all the nodes' (>= lft) left and right by 1.
fn increment_lft_impl(mut albums: Vec<AugmentedAlbum>, id: &str) -> Vec<AugmentedAlbum> {
    let Some(lft) = albums.iter().find(|a| a.id == id).map(|a| n(a._lft)) else {
        return albums;
    };

    for a in albums.iter_mut() {
        if n(a._lft) < lft {
            continue;
        }
        a._lft = Some(n(a._lft) + 1);
        a._rgt = Some(n(a._rgt) + 1);
    }
    albums
}

// We increment all the nodes above rgt by 1 and increment rgt by 1.
fn increment_rgt_impl(mut albums: Vec<AugmentedAlbum>, id: &str) -> Vec<AugmentedAlbum> {
    let Some(rgt) = albums.iter().find(|a| a.id == id).map(|a| n(a._rgt)) else {
        return albums;
    };

    for a in albums.iter_mut() {
        let a_rgt = n(a._rgt);
        if a_rgt < rgt {
            continue;
        }
        if a_rgt == rgt {
            a._rgt = Some(a_rgt + 1);
        } else {
            a._lft = Some(n(a._lft) + 1);
            a._rgt = Some(a_rgt + 1);
        }
    }
    albums
}

// We decrement all the nodes above lft by 1.
fn decrement_lft_impl(mut albums: Vec<AugmentedAlbum>, id: &str) -> Vec<AugmentedAlbum> {
    let Some(lft) = albums.iter().find(|a| a.id == id).map(|a| n(a._lft)) else {
        return albums;
    };

    for a in albums.iter_mut() {
        if n(a._lft) < lft {
            continue;
        }
        a._lft = Some(n(a._lft) - 1);
        a._rgt = Some(n(a._rgt) - 1);
    }
    albums
}

// We decrement all the nodes above rgt by 1 and decrement rgt by 1 IF lft > rgt - 1.
fn decrement_rgt_impl(mut albums: Vec<AugmentedAlbum>, id: &str) -> Vec<AugmentedAlbum> {
    let Some(rgt) = albums.iter().find(|a| a.id == id).map(|a| n(a._rgt)) else {
        return albums;
    };

    for a in albums.iter_mut() {
        let a_rgt = n(a._rgt);
        if a_rgt < rgt {
            continue;
        }
        // safety check
        if n(a._lft) == rgt - 1 {
            continue;
        }
        if a_rgt == rgt {
            a._rgt = Some(a_rgt - 1);
        } else {
            // Mirrors the upstream TS composable's `decrementRgt` verbatim,
            // including its else-branch incrementing rather than
            // decrementing (matching `incrementRgt`'s else-branch). This is
            // a faithful port, not a bugfix — flagged for upstream review.
            a._lft = Some(n(a._lft) + 1);
            a._rgt = Some(a_rgt + 1);
        }
    }
    albums
}

fn get_modified_albums_impl(
    current: Vec<AlbumTree>,
    original: Vec<AlbumTree>,
) -> Vec<ModifiedAlbum> {
    let original_map: HashMap<String, AlbumTree> =
        original.into_iter().map(|a| (a.id.clone(), a)).collect();

    current
        .into_iter()
        .filter(|a| match original_map.get(&a.id) {
            None => true,
            Some(orig) => {
                a._lft != orig._lft || a._rgt != orig._rgt || a.parent_id != orig.parent_id
            }
        })
        .map(|a| ModifiedAlbum {
            id: a.id,
            _lft: a._lft,
            _rgt: a._rgt,
            parent_id: a.parent_id,
        })
        .collect()
}

#[wasm_bindgen(typescript_custom_section)]
const TS_APPEND_CONTENT: &'static str = r#"
export interface AlbumTree {
    id: string;
    title: string;
    parent_id: string | null;
    _lft: number | null;
    _rgt: number | null;
}

export interface AugmentedAlbum extends AlbumTree {
    prefix: string;
    trimmedId: string;
    trimmedParentId: string;
    isDuplicate_rgt: boolean;
    isDuplicate_lft: boolean;
    isExpectedParentId: boolean;
}

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
    lft: number | null;
    rgt: number | null;
    parentId: string | null;
}

export interface PrepareResult {
    albums: AugmentedAlbum[];
    errors: ErrorDescriptor[];
    isValid: boolean;
}

export interface ModifiedAlbum {
    id: string;
    _lft: number | null;
    _rgt: number | null;
    parent_id: string | null;
}
"#;

/// Validates a tree: builds duplicate `_lft`/`_rgt` sets, walks the tree in
/// `_lft` order tracking a parent stack to flag rows with an unexpected
/// `parent_id`, and classifies every row that fails validation. `source`
/// should already be sorted by `_lft` (the original composable relies on the
/// same precondition).
#[wasm_bindgen(js_name = prepareAlbums, unchecked_return_type = "PrepareResult")]
pub fn prepare_albums(
    #[wasm_bindgen(unchecked_param_type = "AlbumTree[]")] source: JsValue,
) -> Result<JsValue, JsValue> {
    let source: Vec<AlbumTree> = serde_wasm_bindgen::from_value(source).map_err(err_to_js)?;
    let result = prepare_albums_impl(source);
    serde_wasm_bindgen::to_value(&result).map_err(err_to_js)
}

/// Shifts every album whose `_lft` is `>= id`'s `_lft` up by one, making room
/// to insert immediately before it.
#[wasm_bindgen(js_name = incrementLft, unchecked_return_type = "AugmentedAlbum[]")]
pub fn increment_lft(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbum[]")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums: Vec<AugmentedAlbum> = serde_wasm_bindgen::from_value(albums).map_err(err_to_js)?;
    serde_wasm_bindgen::to_value(&increment_lft_impl(albums, id)).map_err(err_to_js)
}

/// Shifts every album whose `_rgt` is `>= id`'s `_rgt` up by one, making room
/// to insert immediately after it (as a sibling) or as its first child.
#[wasm_bindgen(js_name = incrementRgt, unchecked_return_type = "AugmentedAlbum[]")]
pub fn increment_rgt(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbum[]")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums: Vec<AugmentedAlbum> = serde_wasm_bindgen::from_value(albums).map_err(err_to_js)?;
    serde_wasm_bindgen::to_value(&increment_rgt_impl(albums, id)).map_err(err_to_js)
}

/// Inverse of [`increment_lft`]: closes the gap left by removing an album at
/// `id`'s `_lft`.
#[wasm_bindgen(js_name = decrementLft, unchecked_return_type = "AugmentedAlbum[]")]
pub fn decrement_lft(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbum[]")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums: Vec<AugmentedAlbum> = serde_wasm_bindgen::from_value(albums).map_err(err_to_js)?;
    serde_wasm_bindgen::to_value(&decrement_lft_impl(albums, id)).map_err(err_to_js)
}

/// Inverse of [`increment_rgt`]: closes the gap left by removing an album at
/// `id`'s `_rgt`, unless the safety check (`_lft === _rgt - 1`) trips.
#[wasm_bindgen(js_name = decrementRgt, unchecked_return_type = "AugmentedAlbum[]")]
pub fn decrement_rgt(
    #[wasm_bindgen(unchecked_param_type = "AugmentedAlbum[]")] albums: JsValue,
    id: &str,
) -> Result<JsValue, JsValue> {
    let albums: Vec<AugmentedAlbum> = serde_wasm_bindgen::from_value(albums).map_err(err_to_js)?;
    serde_wasm_bindgen::to_value(&decrement_rgt_impl(albums, id)).map_err(err_to_js)
}

/// Diffs `current` against `original` by id, returning only the rows whose
/// `_lft`, `_rgt` or `parent_id` changed (plus any row in `current` that
/// isn't in `original` at all, i.e. newly added).
#[wasm_bindgen(js_name = getModifiedAlbums, unchecked_return_type = "ModifiedAlbum[]")]
pub fn get_modified_albums(
    #[wasm_bindgen(unchecked_param_type = "AlbumTree[]")] current: JsValue,
    #[wasm_bindgen(unchecked_param_type = "AlbumTree[]")] original: JsValue,
) -> Result<JsValue, JsValue> {
    let current: Vec<AlbumTree> = serde_wasm_bindgen::from_value(current).map_err(err_to_js)?;
    let original: Vec<AlbumTree> = serde_wasm_bindgen::from_value(original).map_err(err_to_js)?;
    serde_wasm_bindgen::to_value(&get_modified_albums_impl(current, original)).map_err(err_to_js)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, parent_id: Option<&str>, lft: i64, rgt: i64) -> AlbumTree {
        AlbumTree {
            id: id.to_string(),
            title: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            _lft: Some(lft),
            _rgt: Some(rgt),
        }
    }

    fn valid_tree() -> Vec<AlbumTree> {
        // root(1,6) -> child-a(2,3), child-b(4,5)
        vec![
            album("root", None, 1, 6),
            album("child-a", Some("root"), 2, 3),
            album("child-b", Some("root"), 4, 5),
        ]
    }

    #[test]
    fn valid_tree_has_no_errors() {
        let result = prepare_albums_impl(valid_tree());
        assert!(result.is_valid);
        assert!(result.errors.is_empty());
        assert_eq!(result.albums[1].prefix, "  │ ");
        assert_eq!(result.albums[0].prefix, "");
    }

    #[test]
    fn detects_duplicate_lft_and_rgt() {
        let mut tree = valid_tree();
        tree[2]._lft = Some(2); // now collides with child-a's _lft
        let result = prepare_albums_impl(tree);
        assert!(!result.is_valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.kind == ErrorKind::DuplicateLeft || e.kind == ErrorKind::DuplicateRight));
    }

    #[test]
    fn detects_null_and_zero_lft_rgt() {
        let mut tree = valid_tree();
        tree[1]._lft = None;
        tree[2]._rgt = Some(0);
        let result = prepare_albums_impl(tree);
        assert!(!result.is_valid);
        let kinds: Vec<_> = result.errors.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&ErrorKind::InvalidLeft));
        assert!(kinds.contains(&ErrorKind::InvalidRight));
    }

    #[test]
    fn detects_lft_gte_rgt() {
        let mut tree = valid_tree();
        tree[1]._lft = Some(3);
        tree[1]._rgt = Some(3);
        let result = prepare_albums_impl(tree);
        assert!(result
            .errors
            .iter()
            .any(|e| e.kind == ErrorKind::InvalidLeftRight));
    }

    #[test]
    fn detects_unexpected_parent_id() {
        let mut tree = valid_tree();
        tree[1].parent_id = Some("someone-else".to_string());
        let result = prepare_albums_impl(tree);
        assert!(result.errors.iter().any(|e| e.kind == ErrorKind::Parent));
    }

    #[test]
    fn increment_lft_shifts_everything_at_or_after() {
        let result = prepare_albums_impl(valid_tree());
        let shifted = increment_lft_impl(result.albums, "child-b");
        let a = |id: &str| shifted.iter().find(|a| a.id == id).unwrap();
        // Only rows whose _lft >= 4 move; root's _lft is 1, so it (and its
        // _rgt) is left untouched even though 6 >= 4.
        assert_eq!(a("root")._lft, Some(1));
        assert_eq!(a("root")._rgt, Some(6));
        assert_eq!(a("child-a")._lft, Some(2));
        assert_eq!(a("child-b")._lft, Some(5));
        assert_eq!(a("child-b")._rgt, Some(6));
    }

    #[test]
    fn increment_rgt_widens_the_target_only_at_boundary() {
        let result = prepare_albums_impl(valid_tree());
        let shifted = increment_rgt_impl(result.albums, "child-a");
        let a = |id: &str| shifted.iter().find(|a| a.id == id).unwrap();
        assert_eq!(a("child-a")._rgt, Some(4));
        assert_eq!(a("child-a")._lft, Some(2)); // exact rgt match: only rgt moves
        assert_eq!(a("child-b")._lft, Some(5));
        assert_eq!(a("child-b")._rgt, Some(6));
        assert_eq!(a("root")._rgt, Some(7));
    }

    #[test]
    fn decrement_is_inverse_of_increment_for_lft() {
        let result = prepare_albums_impl(valid_tree());
        let shifted = increment_lft_impl(result.albums, "child-b");
        let restored = decrement_lft_impl(shifted, "child-b");
        let a = |id: &str| restored.iter().find(|a| a.id == id).unwrap();
        assert_eq!(a("root")._lft, Some(1));
        assert_eq!(a("root")._rgt, Some(6));
        assert_eq!(a("child-b")._lft, Some(4));
        assert_eq!(a("child-b")._rgt, Some(5));
    }

    #[test]
    fn unknown_id_leaves_albums_untouched() {
        let result = prepare_albums_impl(valid_tree());
        let untouched = increment_lft_impl(result.albums.clone(), "does-not-exist");
        assert_eq!(
            untouched
                .iter()
                .map(|a| (a._lft, a._rgt))
                .collect::<Vec<_>>(),
            result
                .albums
                .iter()
                .map(|a| (a._lft, a._rgt))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn get_modified_albums_reports_only_changes() {
        let original = valid_tree();
        let mut current = valid_tree();
        current[1]._lft = Some(20);
        current[1]._rgt = Some(21);
        current.push(album("child-c", Some("root"), 30, 31));

        let modified = get_modified_albums_impl(current, original);
        let ids: HashSet<_> = modified.iter().map(|m| m.id.clone()).collect();
        assert_eq!(
            ids,
            HashSet::from(["child-a".to_string(), "child-c".to_string()])
        );
    }
}
