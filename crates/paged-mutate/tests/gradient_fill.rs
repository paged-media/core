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

//! B-03 verification (plugin platform, decision 2026-06-06) —
//! gradient ASSIGNMENT through the mutation layer. The open question
//! was whether `SetProperty { FrameFillColor, ColorRef("Gradient/…") }`
//! is accepted and actually RENDERS as a gradient (the apply arm is a
//! plain ref assignment; the render path resolves gradient ids via
//! `color_id_to_paint_with_list_dir`). This test pins the full
//! round trip: create gradient → assign to a rectangle fill →
//! display list carries a gradient paint → inverse restores.

use std::path::PathBuf;

use paged_compose::{DisplayCommand, Paint};
use paged_mutate::{apply, GradientSpec, GradientStopSpec, NodeId, Operation, PropertyPath, Value};
use paged_renderer::pipeline::{build_document, PipelineOptions};

fn fixture_bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry.idml");
    std::fs::read(path).expect("read geometry fixture")
}

fn gradient_paint_count(commands: &[DisplayCommand]) -> usize {
    commands
        .iter()
        .filter(|c| {
            matches!(
                c,
                DisplayCommand::FillPath {
                    paint: Paint::LinearGradient(_) | Paint::RadialGradient(_),
                    ..
                } | DisplayCommand::FillPathBlend {
                    paint: Paint::LinearGradient(_) | Paint::RadialGradient(_),
                    ..
                }
            )
        })
        .count()
}

#[test]
fn gradient_assignment_round_trips_to_a_gradient_paint() {
    let bytes = fixture_bytes();
    let mut doc = paged_parse::import_idml_doc(&bytes).expect("open document");

    // First rectangle in the document — the assignment target.
    let (rect_id, prev_fill) = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .filter_map(|r| r.self_id.clone().map(|id| (id, r.fill_color.clone())))
        .next()
        .expect("fixture has a rectangle with a self id");

    // Baseline: how many gradient fills does the document already
    // paint? (The geometry fixture may legitimately contain some.)
    let before = build_document(&doc, &PipelineOptions::default()).expect("build before");
    let baseline: usize = before
        .pages
        .iter()
        .map(|p| gradient_paint_count(&p.list.commands))
        .sum();

    // 1 · Create a gradient with an explicit id.
    let created = apply(
        &mut doc,
        &Operation::CreateGradient {
            spec: GradientSpec {
                self_id: Some("Gradient/test-b03".to_string()),
                name: Some("B03 verification".to_string()),
                kind: "Linear".to_string(),
                stops: vec![
                    GradientStopSpec {
                        stop_color: "Color/Black".to_string(),
                        location_pct: 0.0,
                        midpoint_pct: None,
                    },
                    // Same swatch on both stops: paged-gen fixtures
                    // carry ONLY the colors they use, and a stop
                    // referencing a missing swatch makes the renderer
                    // drop the WHOLE fill silently (sharp edge worth
                    // its own diagnostic someday — recorded in the
                    // B-03 close-out). Degenerate ramp, valid paint.
                    GradientStopSpec {
                        stop_color: "Color/Black".to_string(),
                        location_pct: 100.0,
                        midpoint_pct: None,
                    },
                ],
            },
        },
    )
    .expect("create gradient");

    // 2 · Assign it to the rectangle's fill — the B-03 question.
    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some("Gradient/test-b03".to_string())),
        },
    )
    .expect("assign gradient fill");

    let rect = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(rect_id.as_str()))
        .expect("rectangle still present");
    assert_eq!(
        rect.fill_color.as_deref(),
        Some("Gradient/test-b03"),
        "mutation layer accepted the gradient ref"
    );

    // 3 · The render path resolves it: one MORE gradient paint than
    // the baseline.
    let after = build_document(&doc, &PipelineOptions::default()).expect("build after");
    let count: usize = after
        .pages
        .iter()
        .map(|p| gradient_paint_count(&p.list.commands))
        .sum();
    assert_eq!(
        count,
        baseline + 1,
        "assigned gradient must surface as a gradient paint in the display list"
    );

    // 4 · The inverse restores the previous fill exactly.
    apply(&mut doc, &applied.inverse).expect("undo assignment");
    let rect = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(rect_id.as_str()))
        .expect("rectangle still present");
    assert_eq!(rect.fill_color, prev_fill, "inverse restores prior fill");

    // 5 · Cleanup symmetry: deleting the gradient inverts too.
    apply(&mut doc, &created.inverse).expect("undo gradient creation");
}
