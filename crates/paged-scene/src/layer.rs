/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! Shared layer-rule helpers.
//!
//! IDML `<Layer>` elements gate two behaviors that the renderer and the
//! canvas hit-tester must agree on exactly — otherwise selection and
//! rendering can disagree about which element is on top. This module
//! is the single source of truth for visibility, locked state, and
//! z-position.
//!
//! Two flavors of accessor:
//!   - `build_*_map` returns a `HashMap` keyed by layer `self_id`,
//!     intended for hot loops (renderer pipeline scans every page item).
//!   - `layer_*` is a one-shot query, intended for cold paths
//!     (click-hit, marquee), where the linear scan over the typically
//!     <10 layers is fine.
//!
//! Items without an explicit `ItemLayer` are treated as on the default
//! layer — visible, not locked — matching InDesign's behavior.
//!
//! ## Z-order
//!
//! IDML lists layers top-first (`layers[0]` = topmost), so the z-index
//! returned by `layer_z_index` is **lower = higher z**. Sorting items
//! by ascending z-index walks them top-to-bottom in paint order.

use std::collections::HashMap;

use paged_parse::DesignMap;

/// True iff `layer_id` AND every ancestor layer (via `parent_id`) is
/// visible-and-printable. A layer nested inside a hidden or
/// non-printable group (folder) is itself suppressed, matching
/// InDesign's Layers panel. An unknown / dangling id resolves to `true`
/// (visible) — same default as a missing `ItemLayer`. Bounded by the
/// layer count so a malformed parent cycle terminates.
fn chain_render_visible(designmap: &DesignMap, layer_id: &str) -> bool {
    let mut id = layer_id.to_string();
    let mut guard = designmap.layers.len() + 1;
    loop {
        if guard == 0 {
            return true;
        }
        guard -= 1;
        match designmap.layers.iter().find(|l| l.self_id == id) {
            Some(l) => {
                if !(l.visible && l.printable) {
                    return false;
                }
                match &l.parent_id {
                    Some(p) => id = p.clone(),
                    None => return true,
                }
            }
            None => return true,
        }
    }
}

/// True iff `layer_id` OR any ancestor layer is locked. A layer nested
/// inside a locked group is itself locked. Unknown id ⇒ unlocked.
fn chain_locked(designmap: &DesignMap, layer_id: &str) -> bool {
    let mut id = layer_id.to_string();
    let mut guard = designmap.layers.len() + 1;
    loop {
        if guard == 0 {
            return false;
        }
        guard -= 1;
        match designmap.layers.iter().find(|l| l.self_id == id) {
            Some(l) => {
                if l.locked {
                    return true;
                }
                match &l.parent_id {
                    Some(p) => id = p.clone(),
                    None => return false,
                }
            }
            None => return false,
        }
    }
}

/// Precompute the `layer_id → (visible && printable)` map used by the
/// renderer to skip suppressed items. The value folds in the ancestor
/// chain (a visible child inside a hidden group is `false`). Lookup
/// defaults to `true` for items with no `ItemLayer` ref or a ref that
/// doesn't resolve.
pub fn build_layer_render_map(designmap: &DesignMap) -> HashMap<&str, bool> {
    designmap
        .layers
        .iter()
        .map(|l| {
            (
                l.self_id.as_str(),
                chain_render_visible(designmap, &l.self_id),
            )
        })
        .collect()
}

/// Precompute the `layer_id → locked` map used by the canvas selection
/// layer to filter out items the user must not be able to grab. Folds
/// in the ancestor chain (a child inside a locked group is locked).
/// The renderer ignores this; it is a selection-layer concern.
pub fn build_layer_locked_map(designmap: &DesignMap) -> HashMap<&str, bool> {
    designmap
        .layers
        .iter()
        .map(|l| (l.self_id.as_str(), chain_locked(designmap, &l.self_id)))
        .collect()
}

/// Build `layer_id → z-position` (0 = topmost). Used to sort items
/// across kinds into the same paint order the renderer follows.
pub fn layer_z_index(designmap: &DesignMap) -> HashMap<&str, usize> {
    designmap
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l.self_id.as_str(), i))
        .collect()
}

/// Lookup against a precomputed `build_layer_render_map`. Pulled out
/// so callers that already hold the map can stay branch-free.
pub fn lookup_layer_render_visible(
    map: &HashMap<&str, bool>,
    item_layer_ref: Option<&str>,
) -> bool {
    match item_layer_ref {
        Some(id) => map.get(id).copied().unwrap_or(true),
        None => true,
    }
}

/// Lookup against a precomputed `build_layer_locked_map`. Items with
/// no `ItemLayer` default to unlocked.
pub fn lookup_layer_locked(map: &HashMap<&str, bool>, item_layer_ref: Option<&str>) -> bool {
    match item_layer_ref {
        Some(id) => map.get(id).copied().unwrap_or(false),
        None => false,
    }
}

/// One-shot: is the layer `item_layer_ref` references visible (and
/// printable)? Items with no `ItemLayer` ref default to visible.
pub fn layer_render_visible(designmap: &DesignMap, item_layer_ref: Option<&str>) -> bool {
    match item_layer_ref {
        Some(id) => chain_render_visible(designmap, id),
        None => true,
    }
}

/// One-shot: is the layer `item_layer_ref` references locked? Items
/// with no `ItemLayer` default to unlocked.
pub fn layer_locked(designmap: &DesignMap, item_layer_ref: Option<&str>) -> bool {
    match item_layer_ref {
        Some(id) => chain_locked(designmap, id),
        None => false,
    }
}

/// One-shot: z-position of the layer `item_layer_ref` references
/// (0 = topmost). Returns `usize::MAX` for unknown / missing refs so
/// stable sorts keep those items in their original document order
/// (below everything that does resolve).
pub fn layer_z(designmap: &DesignMap, item_layer_ref: Option<&str>) -> usize {
    match item_layer_ref {
        Some(id) => designmap
            .layers
            .iter()
            .position(|l| l.self_id == id)
            .unwrap_or(usize::MAX),
        None => usize::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_parse::designmap::Layer;

    fn dm(layers: Vec<Layer>) -> DesignMap {
        DesignMap {
            layers,
            ..DesignMap::default()
        }
    }

    fn layer(id: &str, visible: bool, locked: bool, printable: bool) -> Layer {
        Layer {
            self_id: id.to_string(),
            name: None,
            visible,
            locked,
            printable,
            parent_id: None,
        }
    }

    fn child_layer(id: &str, parent: &str, visible: bool, locked: bool, printable: bool) -> Layer {
        Layer {
            parent_id: Some(parent.to_string()),
            ..layer(id, visible, locked, printable)
        }
    }

    #[test]
    fn visible_default_for_unknown_layer() {
        let d = dm(vec![]);
        assert!(layer_render_visible(&d, Some("missing")));
        assert!(layer_render_visible(&d, None));
    }

    #[test]
    fn hidden_layer_blocks_render() {
        let d = dm(vec![layer("a", false, false, true)]);
        assert!(!layer_render_visible(&d, Some("a")));
    }

    #[test]
    fn non_printable_layer_blocks_render() {
        let d = dm(vec![layer("a", true, false, false)]);
        assert!(!layer_render_visible(&d, Some("a")));
    }

    #[test]
    fn locked_layer_reports_locked() {
        let d = dm(vec![layer("a", true, true, true)]);
        assert!(layer_locked(&d, Some("a")));
        // A locked layer still renders (lock blocks editing, not display);
        // "a" is visible + printable, so render-visibility stays true.
        assert!(layer_render_visible(&d, Some("a")));
    }

    #[test]
    fn z_index_is_top_first() {
        let d = dm(vec![
            layer("top", true, false, true),
            layer("mid", true, false, true),
            layer("bot", true, false, true),
        ]);
        let z = layer_z_index(&d);
        assert_eq!(z["top"], 0);
        assert_eq!(z["mid"], 1);
        assert_eq!(z["bot"], 2);
        assert_eq!(layer_z(&d, Some("top")), 0);
        assert_eq!(layer_z(&d, Some("missing")), usize::MAX);
        assert_eq!(layer_z(&d, None), usize::MAX);
    }

    #[test]
    fn child_hidden_by_ancestor() {
        // A visible+printable child layer inside a hidden parent group
        // must resolve hidden; inside a non-printable group too. A
        // visible child of a visible group stays visible.
        let d = dm(vec![
            layer("grp_hidden", false, false, true),
            child_layer("child_vis", "grp_hidden", true, false, true),
            layer("grp_noprint", true, false, false),
            child_layer("child2", "grp_noprint", true, false, true),
            layer("grp_ok", true, false, true),
            child_layer("child_ok", "grp_ok", true, false, true),
        ]);
        assert!(!layer_render_visible(&d, Some("child_vis")));
        assert!(!layer_render_visible(&d, Some("child2")));
        assert!(layer_render_visible(&d, Some("child_ok")));
        // The precomputed map must agree with the one-shot.
        let render = build_layer_render_map(&d);
        assert!(!render["child_vis"]);
        assert!(!render["child2"]);
        assert!(render["child_ok"]);
    }

    #[test]
    fn child_locked_by_ancestor() {
        // A child layer inside a locked group is locked even if its own
        // Locked flag is false.
        let d = dm(vec![
            layer("grp_locked", true, true, true),
            child_layer("child", "grp_locked", true, false, true),
        ]);
        assert!(layer_locked(&d, Some("child")));
        let locked = build_layer_locked_map(&d);
        assert!(locked["child"]);
    }

    #[test]
    fn precomputed_maps_match_oneshot() {
        let d = dm(vec![
            layer("a", true, false, true),
            layer("b", false, true, false),
        ]);
        let render = build_layer_render_map(&d);
        let locked = build_layer_locked_map(&d);
        assert_eq!(
            lookup_layer_render_visible(&render, Some("a")),
            layer_render_visible(&d, Some("a")),
        );
        assert_eq!(
            lookup_layer_render_visible(&render, Some("b")),
            layer_render_visible(&d, Some("b")),
        );
        assert_eq!(
            lookup_layer_locked(&locked, Some("b")),
            layer_locked(&d, Some("b")),
        );
        assert_eq!(
            lookup_layer_render_visible(&render, None),
            layer_render_visible(&d, None),
        );
    }
}
