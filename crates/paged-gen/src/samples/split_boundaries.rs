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

//! `split-boundaries.idml` — where one block of split-column paragraphs
//! ends and the next begins, against InDesign: consecutive split
//! paragraphs whose settings differ (count, gutters, min space), a split
//! next to a span, identical settings spelled differently, and a block
//! boundary that falls across a column or a frame.
//!
//! The page layout is `span-columns`' (one case per page, one story
//! threaded through frames A and B, ten Inter 10/12 lines per column,
//! numbered one-line paragraphs); see [`super::span_columns`].

use super::span_columns::{
    body, build_cases, heading, para, Case, Spec, SINGLE, SPAN_ALL, SPLIT, SPLIT2,
};
use crate::package::Sample;

pub const SAMPLE: &str = "split-boundaries";

const SPLIT3: Spec = (SPLIT, Some(("short", "3")));
/// [`SPLIT2`] with InDesign's default gutters spelled out.
const SPLIT2_EXPLICIT: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SplitColumnInsideGutter", "6"),
        ("SplitColumnOutsideGutter", "0"),
    ],
    Some(("short", "2")),
);
const SPLIT2_INSIDE20: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SplitColumnInsideGutter", "20"),
    ],
    Some(("short", "2")),
);
const SPLIT2_OUTSIDE10: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SplitColumnOutsideGutter", "10"),
    ],
    Some(("short", "2")),
);
const SPLIT2_MIN_SPACE: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SpanColumnMinSpaceBefore", "6"),
        ("SpanColumnMinSpaceAfter", "12"),
    ],
    Some(("short", "2")),
);
const SPLIT2_MIN_AFTER12: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SpanColumnMinSpaceAfter", "12"),
    ],
    Some(("short", "2")),
);
const SPLIT3_MIN_BEFORE6: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SpanColumnMinSpaceBefore", "6"),
    ],
    Some(("short", "3")),
);
const SPLIT2_SPACE_AFTER4: Spec = (
    &[("SpanColumnType", "SplitColumns"), ("SpaceAfter", "4")],
    Some(("short", "2")),
);
const SPLIT3_SPACE_BEFORE3: Spec = (
    &[("SpanColumnType", "SplitColumns"), ("SpaceBefore", "3")],
    Some(("short", "3")),
);

const SPLIT2_MIN_AFTER6: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SpanColumnMinSpaceAfter", "6"),
    ],
    Some(("short", "2")),
);
const SPLIT3_MIN_BEFORE12: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SpanColumnMinSpaceBefore", "12"),
    ],
    Some(("short", "3")),
);
const SPLIT2_SPACE_AFTER10: Spec = (
    &[("SpanColumnType", "SplitColumns"), ("SpaceAfter", "10")],
    Some(("short", "2")),
);
const SPLIT3_SPACE_BEFORE3_MIN10: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SpaceBefore", "3"),
        ("SpanColumnMinSpaceBefore", "10"),
    ],
    Some(("short", "3")),
);
const SINGLE_SPACE_AFTER5: Spec = (&[("SpaceAfter", "5")], None);
const SINGLE_SPACE_BEFORE5: Spec = (&[("SpaceBefore", "5")], None);

/// The cases, in page order.
pub fn cases() -> Vec<Case> {
    let cat = |parts: Vec<Vec<super::span_columns::Para>>| {
        parts.into_iter().flatten().collect::<Vec<_>>()
    };
    vec![
        Case {
            name: "split 2 then split 3 · 1 column",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2),
                body(6..=11, SPLIT3),
                body(12..=13, SINGLE),
            ]),
        },
        Case {
            name: "split 3 then split 2 · 1 column",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=7, SPLIT3),
                body(8..=11, SPLIT2),
                body(12..=13, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · inside gutter 6 then 20",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=4, SPLIT2),
                body(5..=7, SPLIT2_INSIDE20),
                body(8..=9, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · outside gutter 0 then 10",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=4, SPLIT2),
                body(5..=7, SPLIT2_OUTSIDE10),
                body(8..=9, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · same gutters, min space differs",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=4, SPLIT2),
                body(5..=7, SPLIT2_MIN_SPACE),
                body(8..=9, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · identical settings, defaults spelled out",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=4, SPLIT2),
                body(5..=7, SPLIT2_EXPLICIT),
                body(8..=9, SINGLE),
            ]),
        },
        Case {
            name: "split 2 then span All · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2),
                vec![heading(SPAN_ALL)],
                body(6..=11, SINGLE),
            ]),
        },
        Case {
            name: "span All then split 2 · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=2, SINGLE),
                vec![heading(SPAN_ALL)],
                body(3..=6, SPLIT2),
                body(7..=10, SINGLE),
            ]),
        },
        Case {
            name: "split 2 then split 3 · first block crosses into frame B",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=6, SINGLE),
                body(7..=18, SPLIT2),
                body(19..=24, SPLIT3),
                body(25..=26, SINGLE),
            ]),
        },
        Case {
            name: "split 2 then split 3 · first block crosses into column 2",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=6, SINGLE),
                body(7..=16, SPLIT2),
                body(17..=22, SPLIT3),
                body(23..=24, SINGLE),
            ]),
        },
        Case {
            name: "split 2 then split 3 · first block fills frame A",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=6, SINGLE),
                body(7..=14, SPLIT2),
                body(15..=20, SPLIT3),
                body(21..=22, SINGLE),
            ]),
        },
        Case {
            name: "split 2 min after 12 then split 3 min before 6",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2_MIN_AFTER12),
                body(6..=11, SPLIT3_MIN_BEFORE6),
                body(12..=13, SINGLE),
            ]),
        },
        Case {
            name: "split 2 space after 4 then split 3 space before 3",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2_SPACE_AFTER4),
                body(6..=11, SPLIT3_SPACE_BEFORE3),
                body(12..=13, SINGLE),
            ]),
        },
        Case {
            name: "split 2 then split 3, no single text around",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=4, SPLIT2),
                body(5..=10, SPLIT3),
                vec![para("P11".to_string(), SPLIT2)],
            ]),
        },
        Case {
            name: "split 2 of six then span All · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=7, SPLIT2),
                vec![heading(SPAN_ALL)],
                body(8..=11, SINGLE),
            ]),
        },
        Case {
            name: "split 2 opens the story, then span All · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=4, SPLIT2),
                vec![heading(SPAN_ALL)],
                body(5..=8, SINGLE),
            ]),
        },
        Case {
            name: "two above, split 2 of six, span All · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=2, SINGLE),
                body(3..=8, SPLIT2),
                vec![heading(SPAN_ALL)],
                body(9..=12, SINGLE),
            ]),
        },
        Case {
            name: "split 2 then one single, then span All · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2),
                body(6..=6, SINGLE),
                vec![heading(SPAN_ALL)],
                body(7..=10, SINGLE),
            ]),
        },
        Case {
            name: "split 3 of six then span All · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=7, SPLIT3),
                vec![heading(SPAN_ALL)],
                body(8..=11, SINGLE),
            ]),
        },
        Case {
            name: "split 2 of three then span All · 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=4, SPLIT2),
                vec![heading(SPAN_ALL)],
                body(5..=8, SINGLE),
            ]),
        },
        Case {
            name: "split 2 min after 6 then split 3 min before 12",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2_MIN_AFTER6),
                body(6..=11, SPLIT3_MIN_BEFORE12),
                body(12..=13, SINGLE),
            ]),
        },
        Case {
            name: "split 2 space after 10 then split 3 min before 6",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2_SPACE_AFTER10),
                body(6..=11, SPLIT3_MIN_BEFORE6),
                body(12..=13, SINGLE),
            ]),
        },
        Case {
            name: "split 2 space after 4 then split 3 before 3 min 10",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2_SPACE_AFTER4),
                body(6..=11, SPLIT3_SPACE_BEFORE3_MIN10),
                body(12..=13, SINGLE),
            ]),
        },
        Case {
            name: "single after 5, split 2, single before 5",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE_SPACE_AFTER5),
                body(2..=5, SPLIT2),
                body(6..=6, SINGLE_SPACE_BEFORE5),
                body(7..=7, SINGLE),
            ]),
        },
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    super::span_columns::body_story_id_in(SAMPLE, i)
}

/// Frame `f` ("A" or "B") of case `i`.
pub fn frame_id(f: &str, i: u32) -> String {
    super::span_columns::frame_id_in(SAMPLE, f, i)
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    build_cases(SAMPLE, cases())
}
