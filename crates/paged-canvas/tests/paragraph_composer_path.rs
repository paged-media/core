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

//! `paragraphComposer` on the wire (protocol 65). The model has carried
//! `Composer` and the pipeline has set the Single-line Composer since
//! fc2df8a, but no surface could choose it. Over the `composer` fixture —
//! every case set twice with the same text and measure, once per composer
//! — switching one twin's composer must give exactly the other twin's
//! line breaks, at paragraph level and at style level, and undo must give
//! them back.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation};
use paged_gen::samples::composer::{body_story_id, cases};
use paged_mutate::{PropertyPath as P, StyleCollection, Value as V};

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn composer_model() -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::composer::build()).expect("idml");
    CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![inter_font()],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

/// A story's line breaks: `(paragraph, byte range)` per line. The twins
/// carry the same text, so equal breaks mean the same composition.
fn breaks(m: &CanvasModel, story: &str) -> Vec<(u32, std::ops::Range<u32>)> {
    m.built()
        .story_layout(story)
        .iter()
        .map(|l| (l.paragraph_idx, l.byte_range.clone()))
        .collect()
}

fn whole_story(m: &CanvasModel, story: &str) -> ElementId {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .unwrap_or_else(|| panic!("story {story} not in scene"));
    let chars: u32 = s
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.chars().count() as u32)
        .sum();
    ElementId::StoryRange {
        story_id: story.to_string(),
        start: 0,
        end: chars,
    }
}

fn composer_entry(m: &CanvasModel, id: &ElementId) -> Option<V> {
    m.element_properties(id)
        .expect("properties")
        .entries
        .into_iter()
        .find(|e| e.path == P::ParagraphComposer)
        .expect("the composer is read back")
        .value
}

/// The first unstyled case (composer set on the paragraph) and the first
/// styled one (composer set on its paragraph style) whose twins break
/// differently — the cases where the composer is visible.
fn differing_cases(m: &CanvasModel) -> (u32, u32) {
    let all = cases();
    let differs = |i: u32| breaks(m, &body_story_id(i, 0)) != breaks(m, &body_story_id(i, 1));
    let styled = |i: usize| all[i].word_spacing.is_some() || all[i].letter_spacing.is_some();
    let plain = (0..all.len())
        .find(|&i| !styled(i) && differs(i as u32))
        .expect("an unstyled case where the composers differ");
    let with_style = (0..all.len())
        .find(|&i| styled(i) && differs(i as u32))
        .expect("a styled case where the composers differ");
    (plain as u32, with_style as u32)
}

#[test]
fn paragraph_composer_over_the_wire_recomposes_and_undoes() {
    let mut m = composer_model();
    let (case, _) = differing_cases(&m);
    let paragraph_twin = body_story_id(case, 0);
    let single_twin = body_story_id(case, 1);
    let before = breaks(&m, &paragraph_twin);
    let single = breaks(&m, &single_twin);
    let range = whole_story(&m, &paragraph_twin);
    assert_eq!(
        composer_entry(&m, &range),
        Some(V::Text("HL Composer".into())),
        "the authored composer reads back as its IDML name"
    );

    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: range.clone(),
        path: P::ParagraphComposer,
        value: V::Text("HL Single".into()),
    })
    .expect("set HL Single");
    assert_eq!(
        breaks(&m, &paragraph_twin),
        single,
        "the Paragraph-Composer twin now breaks like the Single-line twin"
    );
    assert_eq!(
        composer_entry(&m, &range),
        Some(V::Text("HL Single".into()))
    );

    m.undo().expect("undo");
    assert_eq!(breaks(&m, &paragraph_twin), before, "undo recomposes");
    assert_eq!(
        composer_entry(&m, &range),
        Some(V::Text("HL Composer".into()))
    );

    // "" clears the override: the paragraph inherits, and nothing above
    // it names a composer, so it is the Paragraph Composer again.
    let single_range = whole_story(&m, &single_twin);
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: single_range.clone(),
        path: P::ParagraphComposer,
        value: V::Text(String::new()),
    })
    .expect("clear");
    assert_eq!(
        composer_entry(&m, &single_range),
        Some(V::Text(String::new()))
    );
    assert_eq!(
        breaks(&m, &single_twin),
        before,
        "an inheriting paragraph is set by the Paragraph Composer"
    );
    m.undo().expect("undo the clear");
    assert_eq!(breaks(&m, &single_twin), single);
}

#[test]
fn an_unknown_composer_is_refused_on_the_wire() {
    let mut m = composer_model();
    let story = body_story_id(0, 0);
    let range = whole_story(&m, &story);
    let before = breaks(&m, &story);
    for bad in ["Adobe Single-line Composer", "HL single", "Knuth"] {
        let err = m.apply_mutation(&Mutation::SetElementProperty {
            element_id: range.clone(),
            path: P::ParagraphComposer,
            value: V::Text(bad.into()),
        });
        assert!(err.is_err(), "{bad:?} must be refused");
    }
    // Not a string at all.
    assert!(m
        .apply_mutation(&Mutation::SetElementProperty {
            element_id: range.clone(),
            path: P::ParagraphComposer,
            value: V::Bool(true),
        })
        .is_err());
    // Inside a batch, the whole batch is refused before any child runs.
    assert!(m
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::SetElementProperty {
                    element_id: range.clone(),
                    path: P::ParagraphComposer,
                    value: V::Text("HL Single".into()),
                },
                Mutation::SetElementProperty {
                    element_id: range.clone(),
                    path: P::ParagraphComposer,
                    value: V::Text("HL Mystery".into()),
                },
            ],
        })
        .is_err());
    assert_eq!(
        composer_entry(&m, &range),
        Some(V::Text("HL Composer".into()))
    );
    assert_eq!(breaks(&m, &story), before);
}

#[test]
fn paragraph_style_composer_cascades_and_undoes() {
    let mut m = composer_model();
    let (_, case) = differing_cases(&m);
    let paragraph_twin = body_story_id(case, 0);
    let before = breaks(&m, &paragraph_twin);
    let single = breaks(&m, &body_story_id(case, 1));
    let style = format!("ParagraphStyle/Composer Case {case} 0");
    let set = |m: &mut CanvasModel, value: V| {
        m.apply_mutation(&Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: style.clone(),
            path: P::ParagraphComposer,
            value,
        })
    };

    set(&mut m, V::Text("HL Single".into())).expect("style set");
    assert_eq!(
        m.scene().styles.paragraph_styles[&style].composer,
        Some(paged_model::Composer::SingleLine)
    );
    assert_eq!(
        breaks(&m, &paragraph_twin),
        single,
        "the style's composer cascades to its paragraphs"
    );

    // Refused at style level too, and the style keeps its value.
    assert!(set(&mut m, V::Text("HL Nonsense".into())).is_err());
    assert_eq!(
        m.scene().styles.paragraph_styles[&style].composer,
        Some(paged_model::Composer::SingleLine)
    );

    m.undo().expect("undo the style edit");
    assert_eq!(
        m.scene().styles.paragraph_styles[&style].composer,
        Some(paged_model::Composer::Paragraph)
    );
    assert_eq!(breaks(&m, &paragraph_twin), before);

    // The World-Ready names are accepted and kept verbatim.
    for (name, composer) in [
        (
            "HL Composer Optyca",
            paged_model::Composer::WorldReadyParagraph,
        ),
        (
            "HL Single Optyca",
            paged_model::Composer::WorldReadySingleLine,
        ),
    ] {
        set(&mut m, V::Text(name.into())).expect(name);
        assert_eq!(
            m.scene().styles.paragraph_styles[&style].composer,
            Some(composer)
        );
    }
}
