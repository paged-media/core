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

//! Complex-script shaping — the first tests in this project to shape
//! anything that isn't Latin.
//!
//! Until 2026-08-19 `corpus/fonts/` held eleven Latin-only static TTFs,
//! so nothing could exercise the part of the shaper that actually earns
//! its keep. The engine has carried bidi machinery for a long time
//! (`apply_bidi_reorder`, Hebrew/Arabic/Syriac/Thaana/NKo ranges), but
//! its tests build synthetic lines with fake glyph ids — they verify
//! REORDERING, never SHAPING. And the whole shaping stack was migrated
//! from rustybuzz to harfrust on 2026-08-18 validated entirely on Latin,
//! which is the one script a HarfBuzz-lineage shaper cannot get wrong.
//!
//! Arabic is the sharpest available probe: a letter takes a different
//! glyph depending on its position in the word (isolated / initial /
//! medial / final), so contextual substitution is observable purely
//! through glyph ids, with no rendering and no golden image.
//!
//! Font: Noto Sans Arabic (OFL, variable `wdth`+`wght`), which also
//! makes it the first variable font in the corpus.
//!
//! Two facts about the shaper's output that these tests encode, because
//! a naive "one char, one glyph" expectation fails on both:
//!
//! 1. **Marks are separate glyphs.** Noto decomposes an Arabic letter
//!    into a base plus its dots; the dot arrives as a ZERO-ADVANCE glyph
//!    positioned by mark attachment. So glyph count > char count, and
//!    the positional form lives on the base glyph.
//! 2. **RTL output is in VISUAL order.** Clusters therefore DESCEND
//!    (4, 2, 0 for a three-letter word), which is correct and is what
//!    the renderer consumes.

use paged_text::{shape_run, Face};

fn arabic_face_bytes() -> Option<Vec<u8>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts/NotoSansArabic-VF.ttf");
    std::fs::read(path).ok()
}

/// ARABIC LETTER BEH — a "connecting" letter, so it has all four forms.
const BEH: char = '\u{628}';
/// ARABIC LETTER ALEF — connects only on its right, so a following
/// letter cannot take a medial form after it.
const ALEF: char = '\u{627}';

/// Base glyphs only — the zero-advance entries are mark glyphs (dots),
/// which carry no positional form.
fn bases(run: &paged_text::ShapedRun) -> Vec<u32> {
    run.glyphs
        .iter()
        .filter(|g| g.x_advance > 0)
        .map(|g| g.glyph_id)
        .collect()
}

#[test]
fn arabic_letters_take_positional_forms() {
    let Some(bytes) = arabic_face_bytes() else {
        panic!(
            "corpus/fonts/NotoSansArabic-VF.ttf missing — complex-script shaping cannot be tested"
        );
    };
    let face = Face::from_slice(&bytes, 0).expect("Noto Sans Arabic parses");

    // The SAME character, shaped alone and inside a word. A shaper that
    // ignores the Arabic joining model returns the same glyph for both;
    // a correct one returns the isolated form for the first and an
    // initial/medial form for the second.
    let isolated = shape_run(&face, &BEH.to_string(), 16.0);
    let word: String = [BEH, BEH, BEH].iter().collect();
    let joined = shape_run(&face, &word, 16.0);

    let iso = bases(&isolated);
    let gids = bases(&joined);
    assert_eq!(iso.len(), 1, "one letter, one base glyph (got {iso:?})");
    assert_eq!(
        gids.len(),
        3,
        "three letters, three base glyphs (got {gids:?})"
    );

    // In ببب the three behs are final, medial and initial — three
    // DIFFERENT glyphs, none of them the isolated form.
    let iso_gid = iso[0];
    assert!(
        gids.iter().all(|g| *g != iso_gid),
        "no beh inside a word may reuse the isolated glyph {iso_gid} (got {gids:?}) \
         — the shaper is not applying Arabic contextual substitution"
    );
    let mut distinct = gids.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        3,
        "final/medial/initial must be three distinct glyphs (got {gids:?})"
    );
}

#[test]
fn alef_blocks_the_following_letter_from_a_medial_form() {
    let Some(bytes) = arabic_face_bytes() else {
        panic!("corpus/fonts/NotoSansArabic-VF.ttf missing");
    };
    let face = Face::from_slice(&bytes, 0).expect("Noto Sans Arabic parses");

    // Alef joins only to its right. So in beh-alef-beh the LAST beh
    // cannot be medial — it must start a new joining group. This is the
    // joining-type rule, not mere "is it a different glyph" logic, and a
    // naive per-character mapper gets it wrong.
    let text: String = [BEH, ALEF, BEH].iter().collect();
    let gids = bases(&shape_run(&face, &text, 16.0));
    assert_eq!(
        gids.len(),
        3,
        "three letters, three base glyphs (got {gids:?})"
    );

    // Output is VISUAL order for RTL, so the logically-last beh (the one
    // after the alef) is reported FIRST.
    let after_alef = gids[0];
    let standalone = bases(&shape_run(&face, &BEH.to_string(), 16.0))[0];
    assert_eq!(
        after_alef, standalone,
        "a beh following alef starts a new joining group, so it takes the \
         ISOLATED form — got {after_alef}, isolated is {standalone}"
    );
}

#[test]
fn shaping_advances_are_positive_and_clusters_map_back_to_bytes() {
    let Some(bytes) = arabic_face_bytes() else {
        panic!("corpus/fonts/NotoSansArabic-VF.ttf missing");
    };
    let face = Face::from_slice(&bytes, 0).expect("Noto Sans Arabic parses");
    let text: String = [BEH, BEH, ALEF].iter().collect();
    let shaped = shape_run(&face, &text, 16.0);

    assert!(shaped.total_advance > 0, "shaped Arabic must have width");
    assert!(
        shaped.glyphs.len() > bases(&shaped).len(),
        "Noto decomposes Arabic dots into zero-advance MARK glyphs; seeing none \
         means mark attachment was lost (glyphs {:?})",
        shaped.glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>()
    );
    for g in shaped.glyphs.iter().filter(|g| g.x_advance > 0) {
        assert!(
            g.x_advance > 0,
            "every Arabic BASE glyph advances (glyph {} advanced {})",
            g.glyph_id,
            g.x_advance
        );
        // Clusters are byte offsets into the input; Arabic is 2 bytes per
        // char in UTF-8, so every cluster must land on a char boundary.
        assert!(
            text.is_char_boundary(g.cluster as usize),
            "cluster {} is not a char boundary in a 2-byte-per-char string",
            g.cluster
        );
    }
}
