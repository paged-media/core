//! Geometry adapters used by the paint modules.
//!
//! Modules that emit paint (fill, stroke) shouldn't switch on
//! [`Geometry`] themselves — the dispatch is centralised here so a
//! future shape variant lands as a single match-arm addition rather
//! than a sweep across every paint module.

use idml_compose::{
    emit_ellipse_transformed_blend, emit_rect_transformed_blend, emit_stroke_ellipse_transformed,
    emit_stroke_rect_transformed, BlendMode, DisplayCommand, Paint, PathId, Stroke, Transform,
};

use super::Geometry;
use crate::pipeline::BuiltPage;

/// Emit a filled primitive for `geom`. When `fill_path` is set
/// (rounded rectangle or interned polygon), the path takes
/// precedence over the geometry's own primitive — that's how
/// `corner_path_module` and the polygon orchestrator hand a
/// pre-interned `PathId` over to the fill module.
pub(crate) fn emit_filled(
    geom: &Geometry<'_>,
    page: &mut BuiltPage,
    paint: Paint,
    blend: BlendMode,
    outer: Transform,
    fill_path: Option<PathId>,
) {
    if let Some(path_id) = fill_path {
        emit_fill_path(page, path_id, paint, blend, outer);
        return;
    }
    match geom {
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } => {
            emit_rect_transformed_blend(*rect, outer, paint, blend, &mut page.list);
        }
        Geometry::Oval { rect } => {
            emit_ellipse_transformed_blend(*rect, outer, paint, blend, &mut page.list);
        }
        Geometry::Polygon { .. } => {
            debug_assert!(
                false,
                "Polygon fill must be routed through fill_path; orchestrator missed an intern step",
            );
        }
        Geometry::Line { .. } => {} // lines have no fill
    }
}

/// Emit a stroked primitive for `geom`. When `stroke_path` is set
/// (the rect-rounded inset path or polygon), it takes precedence.
/// Otherwise we use the shape's natural stroke primitive.
pub(crate) fn emit_stroked(
    geom: &Geometry<'_>,
    page: &mut BuiltPage,
    paint: Paint,
    stroke: Stroke,
    outer: Transform,
    stroke_path: Option<PathId>,
) {
    if let Some(path_id) = stroke_path {
        page.list.commands.push(DisplayCommand::StrokePath {
            path_id,
            paint,
            stroke,
            transform: outer,
        });
        return;
    }
    match geom {
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } => {
            emit_stroke_rect_transformed(*rect, outer, stroke, paint, &mut page.list);
        }
        Geometry::Oval { rect } => {
            emit_stroke_ellipse_transformed(*rect, outer, stroke, paint, &mut page.list);
        }
        Geometry::Polygon { .. } => {
            debug_assert!(
                false,
                "Polygon stroke must be routed through stroke_path; orchestrator missed an intern step",
            );
        }
        Geometry::Line { .. } => {
            debug_assert!(
                false,
                "Lines emit through pipeline::emit_line directly; stroke module shouldn't be reached",
            );
        }
    }
}

/// Push a `FillPath` or `FillPathBlend` depending on the blend mode.
/// Helper kept distinct so callers (the polygon orchestrator + the
/// rounded-rect orchestrator) can reuse it without going through
/// `emit_filled`'s match.
pub(crate) fn emit_fill_path(
    page: &mut BuiltPage,
    path_id: PathId,
    paint: Paint,
    blend: BlendMode,
    outer: Transform,
) {
    if matches!(blend, BlendMode::Normal) {
        page.list.commands.push(DisplayCommand::FillPath {
            path_id,
            paint,
            transform: outer,
        });
    } else {
        page.list.commands.push(DisplayCommand::FillPathBlend {
            path_id,
            paint,
            transform: outer,
            blend_mode: blend,
        });
    }
}

/// Rewrite the tail of `page.list.commands` from `start` onward so that
/// any `FillPath` / `StrokePath` commands become their
/// `*Overprint` counterparts. Used by the orchestrator to apply a
/// frame's `OverprintFill` / `OverprintStroke` flag without threading
/// the boolean through every emit helper. `FillPathBlend` commands
/// don't get rewritten: a non-Normal blend already produces a darken-
/// like composite of its own (or the user explicitly asked for a
/// different mode), so layering overprint on top would double-darken.
pub(crate) fn rewrite_tail_for_overprint(
    page: &mut BuiltPage,
    start: usize,
    overprint_fill: bool,
    overprint_stroke: bool,
) {
    if !overprint_fill && !overprint_stroke {
        return;
    }
    let cmds = &mut page.list.commands;
    for cmd in cmds.iter_mut().skip(start) {
        match cmd {
            DisplayCommand::FillPath {
                path_id,
                paint,
                transform,
            } if overprint_fill => {
                *cmd = DisplayCommand::FillPathOverprint {
                    path_id: *path_id,
                    paint: *paint,
                    transform: *transform,
                };
            }
            DisplayCommand::StrokePath {
                path_id,
                paint,
                stroke,
                transform,
            } if overprint_stroke => {
                *cmd = DisplayCommand::StrokePathOverprint {
                    path_id: *path_id,
                    paint: *paint,
                    stroke: *stroke,
                    transform: *transform,
                };
            }
            _ => {}
        }
    }
}
