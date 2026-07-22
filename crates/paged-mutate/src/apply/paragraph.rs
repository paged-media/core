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

use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath, Value,
};

// ---------------------------------------------------------------------------
// SDK Phase 3 — paragraph properties addressed by `NodeId::StoryRange`
// ---------------------------------------------------------------------------
//
// Paragraphs are atomic: you can't half-apply `ParagraphSpaceBefore`
// to the middle of a paragraph. The apply layer walks `story.paragraphs`,
// finds every paragraph whose `[para_start, para_end)` intersects the
// requested `[start, end)`, and writes the property to each. Inverse
// is a `Batch` of per-paragraph SetProperty restorations addressed
// at each paragraph's full range — undo applies them in order to
// restore prior values without needing to know the original input
// range. Paragraph boundaries are NOT split (unlike CharacterRuns) —
// the apply layer rounds the range to whole paragraphs by treating
// intersection as the trigger.

pub(super) fn apply_paragraph_property(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
    node: &NodeId,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    if start >= end {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path,
            reason: format!("empty range: start={start} >= end={end}"),
        });
    }

    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;

    let story = &mut doc.stories[story_idx].story;
    let mut inverse_ops: Vec<Operation> = Vec::new();
    let mut char_offset: u32 = 0;

    for para in story.paragraphs.iter_mut() {
        let para_chars: u32 = para
            .runs
            .iter()
            .map(|r| r.text.chars().count() as u32)
            .sum();
        let para_start = char_offset;
        let para_end = char_offset + para_chars;
        char_offset = para_end;

        // Skip paragraphs entirely outside [start, end).
        if para_end <= start || para_start >= end {
            continue;
        }

        let (prev_value, _new_set) = apply_paragraph_field(para, path, value)?;
        inverse_ops.push(Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: para_start,
                end: para_end,
            },
            path,
            value: prev_value,
        });
    }

    if inverse_ops.is_empty() {
        return Ok(AppliedOperation {
            op: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            inverse: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            invalidation: InvalidationHint::default(),
        });
    }

    let invalidation = match doc.frame_for_story.get(story_id) {
        Some(frame) => {
            if let Some(self_id) = &frame.self_id {
                InvalidationHint {
                    text_reflow: vec![NodeId::TextFrame(self_id.clone())],
                    ..Default::default()
                }
            } else {
                InvalidationHint::default()
            }
        }
        None => InvalidationHint::default(),
    };

    let inverse = if inverse_ops.len() == 1 {
        inverse_ops.into_iter().next().unwrap()
    } else {
        Operation::Batch { ops: inverse_ops }
    };

    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path,
            value: value.clone(),
        },
        inverse,
        invalidation,
    })
}

/// W0.2 — set one `Option<f32>` field on a `Paragraph` from a
/// `Value::Length`. `Length(None)` clears the override; the captured
/// prior `Option<f32>` round-trips bytewise through the inverse.
/// Paragraph-scope analogue of `set_run_length_field`.
pub(super) fn set_para_length_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<f32>,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let prev = *slot;
    *slot = *new_val;
    Ok((Value::Length(prev), Value::Length(*new_val)))
}

/// W0.2 — set one `u32` count field on a `Paragraph` from a
/// `Value::Length` carrying the integer (the inspector's
/// integer-as-Length convention). `Length(None)` ⇒ 0. The captured
/// prior is returned as `Value::Length(Some(prev as f32))` so the
/// inverse round-trips bytewise. `field` is a non-`Option` `u32`
/// (the drop-cap counts default to 0, not `None`).
pub(super) fn set_para_u32_length_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut u32,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let prev = *slot;
    // Round defensively; counts are authored as whole numbers but the
    // wire carries f32. Negative / NaN clamps to 0.
    *slot = new_val.map(|n| n.max(0.0).round() as u32).unwrap_or(0);
    Ok((
        Value::Length(Some(prev as f32)),
        Value::Length(Some(*slot as f32)),
    ))
}

/// W0.2 — set one `Option<u32>` count field on a `Paragraph` from a
/// `Value::Length` carrying the integer. `Length(None)` clears the
/// override. The captured prior `Option<u32>` round-trips bytewise.
pub(super) fn set_para_opt_u32_length_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<u32>,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let prev = *slot;
    *slot = new_val.map(|n| n.max(0.0).round() as u32);
    Ok((
        Value::Length(prev.map(|n| n as f32)),
        Value::Length(*new_val),
    ))
}

/// W0.2 — set one `Option<bool>` field on a `Paragraph` from a
/// `Value::Bool`. The write always stores `Some(new_val)`. The
/// inverse captures `prev.unwrap_or(default_when_none)` — a write
/// over an explicit prior round-trips bytewise; a prior-`None` undoes
/// to `Some(default_when_none)` (the `Value::Bool` wire shape carries
/// no `None`). Paragraph-scope analogue of `set_run_bool_field`, with
/// an explicit default so each toggle restores its own IDML default.
pub(super) fn set_para_bool_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<bool>,
    default_when_none: bool,
) -> Result<(Value, Value), OperationError> {
    let Value::Bool(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Bool".to_string(),
        });
    };
    let prev = slot.unwrap_or(default_when_none);
    *slot = Some(*new_val);
    Ok((Value::Bool(prev), Value::Bool(*new_val)))
}

/// W0.2 — set one `Option<String>` field on a `Paragraph` from a
/// `Value::Text`. The empty string clears the override (`None`); the
/// captured prior is returned as `Value::Text` (`None ⇒ ""`).
/// Paragraph-scope analogue of `set_run_text_field`.
pub(super) fn set_para_text_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<String>,
) -> Result<(Value, Value), OperationError> {
    let Value::Text(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Text".to_string(),
        });
    };
    let prev = slot.clone().unwrap_or_default();
    *slot = if new_val.is_empty() {
        None
    } else {
        Some(new_val.clone())
    };
    Ok((Value::Text(prev), Value::Text(new_val.clone())))
}

/// W0.2 — set the whole `ParagraphRule` struct (`rule_above` /
/// `rule_below`) from a `Value::ParagraphRule`. `ParagraphRule(None)`
/// clears the rule to the all-`None` default. The captured prior is
/// returned as a `Value::ParagraphRule(Some(prior))` so the inverse
/// round-trips the rule bytewise. Whole-struct analogue of the
/// `FrameGradientFeather` apply.
pub(super) fn set_para_rule_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut paged_model::ParagraphRule,
) -> Result<(Value, Value), OperationError> {
    let Value::ParagraphRule(new_spec) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "ParagraphRule".to_string(),
        });
    };
    let prev = crate::operation::ParagraphRuleSpec::from_parse(slot);
    *slot = match new_spec {
        Some(spec) => spec.to_parse(),
        None => paged_model::ParagraphRule::default(),
    };
    Ok((
        Value::ParagraphRule(Some(prev)),
        Value::ParagraphRule(new_spec.clone()),
    ))
}

pub(super) fn apply_paragraph_field(
    para: &mut paged_model::Paragraph,
    path: PropertyPath,
    value: &Value,
) -> Result<(Value, Value), OperationError> {
    match path {
        PropertyPath::ParagraphSpaceBefore => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = para.space_before;
            para.space_before = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::ParagraphSpaceAfter => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = para.space_after;
            para.space_after = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::ParagraphFirstLineIndent => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = para.first_line_indent;
            para.first_line_indent = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::AppliedParagraphStyle => {
            // Apply-an-entity. Empty string clears the override.
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = para.paragraph_style.clone().unwrap_or_default();
            para.paragraph_style = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        PropertyPath::ParagraphJustification => {
            // SDK Phase 5 (v1 sweep) — paragraph alignment via the
            // IDML attribute string. Empty value clears the override
            // (`None` ⇒ inherit from style cascade); non-empty parses
            // through `Justification::from_idml` and stores. Unknown
            // strings raise `InvalidValue` (the toggle-group primitive
            // ensures the UI never emits an unknown value).
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = para
                .justification
                .map(|j| j.as_idml().to_string())
                .unwrap_or_default();
            para.justification = if new_val.is_empty() {
                None
            } else {
                match paged_model::Justification::from_idml(new_val) {
                    Some(j) => Some(j),
                    None => {
                        return Err(OperationError::InvalidValue {
                            node: NodeId::StoryRange {
                                story_id: String::new(),
                                start: 0,
                                end: 0,
                            },
                            path,
                            reason: format!("unknown Justification: {new_val:?}"),
                        });
                    }
                }
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        // W0.2 — paragraph indents. `Value::Length(None)` clears the
        // per-paragraph override (inherit from the cascade).
        PropertyPath::ParagraphLeftIndent => {
            set_para_length_field(path, value, &mut para.left_indent)
        }
        PropertyPath::ParagraphRightIndent => {
            set_para_length_field(path, value, &mut para.right_indent)
        }
        // W0.2 — drop-cap counts. The run fields are non-`Option`
        // `u32` (0 ⇒ no drop cap), carried on the wire as
        // integer-Length.
        PropertyPath::ParagraphDropCapCharacters => {
            set_para_u32_length_field(path, value, &mut para.drop_cap_characters)
        }
        PropertyPath::ParagraphDropCapLines => {
            set_para_u32_length_field(path, value, &mut para.drop_cap_lines)
        }
        // W0.2 — keep-with-next is an `Option<u32>` line count.
        PropertyPath::ParagraphKeepWithNext => {
            set_para_opt_u32_length_field(path, value, &mut para.keep_with_next)
        }
        // W0.2 — boolean toggles. Each restores its own IDML default
        // on a prior-`None` undo: hyphenation defaults true,
        // keep-lines-together defaults false.
        PropertyPath::ParagraphHyphenation => {
            set_para_bool_field(path, value, &mut para.hyphenation, true)
        }
        PropertyPath::ParagraphKeepLinesTogether => {
            set_para_bool_field(path, value, &mut para.keep_lines_together, false)
        }
        // W0.2 — whole rule structs.
        PropertyPath::ParagraphRuleAbove => set_para_rule_field(path, value, &mut para.rule_above),
        PropertyPath::ParagraphRuleBelow => set_para_rule_field(path, value, &mut para.rule_below),
        // W0.2 — whole `<TabList>` replacement. The captured prior is
        // returned as a `Value::TabStops` so the inverse restores the
        // exact prior stop list (bytewise round-trip).
        PropertyPath::ParagraphTabStops => {
            let Value::TabStops(new_stops) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "TabStops".to_string(),
                });
            };
            let prev: Vec<crate::operation::TabStopSpec> = para
                .tab_list
                .iter()
                .map(crate::operation::TabStopSpec::from_parse)
                .collect();
            para.tab_list = new_stops.iter().map(|s| s.to_parse()).collect();
            Ok((Value::TabStops(prev), Value::TabStops(new_stops.clone())))
        }
        // W0.2 — bullets / numbering list type. Stored verbatim as the
        // IDML enum string; empty clears the override.
        PropertyPath::ParagraphListType => {
            set_para_text_field(path, value, &mut para.bullets_list_type)
        }
        // W0.2 — bullet glyph. The wire carries the glyph character
        // (`Value::Text`); the run field is a `u32` codepoint. The
        // empty string clears the override; a multi-char string takes
        // the first scalar. The inverse re-encodes the prior codepoint
        // back to its glyph (a prior-`None` round-trips to `""`).
        PropertyPath::ParagraphBulletCharacter => {
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = para
                .bullet_character
                .and_then(char::from_u32)
                .map(|c| c.to_string())
                .unwrap_or_default();
            para.bullet_character = new_val.chars().next().map(|c| c as u32);
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        // W0.2 — numbering-format expression. Stored verbatim; empty
        // clears the override.
        PropertyPath::ParagraphNumberingFormat => {
            set_para_text_field(path, value, &mut para.numbering_format)
        }
        // W1.22 (engine gap 22) — applied numbering list ref. Stored
        // verbatim; empty clears the override (inherit from the style
        // cascade). The renderer resolves it to find the list's
        // cross-story continuity flag.
        PropertyPath::ParagraphAppliedNumberingList => {
            set_para_text_field(path, value, &mut para.applied_numbering_list)
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: NodeId::StoryRange {
                story_id: String::new(),
                start: 0,
                end: 0,
            },
            path,
        }),
    }
}

/// SDK Phase 3.x — split a `CharacterRun` at character offset
/// `char_idx`. The left piece contains the first `char_idx`
/// characters of `run.text`; the right piece contains the rest.
/// Every other field is duplicated via `Clone` so the two pieces
/// inherit identical properties pre-mutation. `char_idx` must lie
/// strictly inside the run (0 < char_idx < run.text.chars().count()) —
/// the caller is responsible for that constraint; this function
/// produces undefined byte boundaries otherwise.
pub(super) fn split_run_at(
    run: paged_model::CharacterRun,
    char_idx: u32,
) -> (paged_model::CharacterRun, paged_model::CharacterRun) {
    // Find the byte position of the char_idx'th character. char_indices
    // yields each char's byte offset; chars past the end map to the
    // string's total byte length.
    let byte_idx = run
        .text
        .char_indices()
        .nth(char_idx as usize)
        .map(|(byte, _)| byte)
        .unwrap_or(run.text.len());
    let left_text = run.text[..byte_idx].to_string();
    let right_text = run.text[byte_idx..].to_string();
    let mut left = run.clone();
    left.text = left_text;
    let mut right = run;
    right.text = right_text;
    (left, right)
}

/// W0.1 — set one `Option<String>` field on a `CharacterRun` from a
/// `Value::Text`. The empty string clears the override (`None`); the
/// captured prior is returned as `Value::Text` (`None ⇒ ""`) so the
/// inverse re-applies the prior string and round-trips a prior-`None`
/// back to `None`. `field` selects the run field by `&mut` reference.
pub(super) fn set_run_text_field(
    run: &mut paged_model::CharacterRun,
    path: PropertyPath,
    value: &Value,
    field: impl FnOnce(&mut paged_model::CharacterRun) -> &mut Option<String>,
) -> Result<(Value, Value), OperationError> {
    let Value::Text(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Text".to_string(),
        });
    };
    let slot = field(run);
    let prev = slot.clone().unwrap_or_default();
    *slot = if new_val.is_empty() {
        None
    } else {
        Some(new_val.clone())
    };
    Ok((Value::Text(prev), Value::Text(new_val.clone())))
}

/// W0.1 — set one `Option<f32>` field on a `CharacterRun` from a
/// `Value::Length`. `Length(None)` clears the override; the captured
/// prior `Option<f32>` round-trips bytewise through the inverse.
pub(super) fn set_run_length_field(
    run: &mut paged_model::CharacterRun,
    path: PropertyPath,
    value: &Value,
    field: impl FnOnce(&mut paged_model::CharacterRun) -> &mut Option<f32>,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let slot = field(run);
    let prev = *slot;
    *slot = *new_val;
    Ok((Value::Length(prev), Value::Length(*new_val)))
}

/// W0.1 — set one `Option<bool>` field on a `CharacterRun` from a
/// `Value::Bool`. The write always stores `Some(new_val)`. The
/// inverse captures `prev.unwrap_or(false)` — a write over an
/// explicit prior round-trips bytewise; a prior-`None` undoes to
/// `Some(false)` (the `Value::Bool` wire shape carries no `None`).
pub(super) fn set_run_bool_field(
    run: &mut paged_model::CharacterRun,
    path: PropertyPath,
    value: &Value,
    default: bool,
    field: impl FnOnce(&mut paged_model::CharacterRun) -> &mut Option<bool>,
) -> Result<(Value, Value), OperationError> {
    let Value::Bool(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Bool".to_string(),
        });
    };
    let slot = field(run);
    // `None` means "inherit the field's default". The inverse must
    // restore the EFFECTIVE prior value, so collapse `None` to the
    // field's default (ligatures default ON; underline/strikethru OFF) —
    // undoing a toggle on a defaulted run lands the visible default back,
    // not the wrong-polarity `false` the old `unwrap_or(false)` produced.
    let prev = slot.unwrap_or(default);
    *slot = Some(*new_val);
    Ok((Value::Bool(prev), Value::Bool(*new_val)))
}

/// Apply one character property to one `CharacterRun`. Returns
/// (previous_value, new_value) on success. The new_value mirrors
/// what was set so downstream logging can attribute correctly even
/// when the caller passes through e.g. `Length(None)`.
pub(super) fn apply_character_field_on_run(
    run: &mut paged_model::CharacterRun,
    path: PropertyPath,
    value: &Value,
) -> Result<(Value, Value), OperationError> {
    match path {
        PropertyPath::CharacterFontSize => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = run.point_size;
            run.point_size = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::CharacterLeading => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = run.leading;
            run.leading = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = run.tracking;
            run.tracking = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "ColorRef".to_string(),
                });
            };
            let prev = run.fill_color.clone();
            run.fill_color = new_val.clone();
            Ok((Value::ColorRef(prev), Value::ColorRef(new_val.clone())))
        }
        PropertyPath::AppliedCharacterStyle => {
            // Apply-an-entity (D3 of panel-catalog doc): the
            // character_style ref is a string-id payload. Empty
            // string clears the override; otherwise stores the
            // style's `self_id`.
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = run.character_style.clone().unwrap_or_default();
            run.character_style = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        PropertyPath::AppliedConditions => {
            // SDK Phase 5 (D3 completion) — applied conditions per
            // CharacterRun. Wire encoding mirrors IDML's
            // `AppliedConditions="A B C"` attribute: a single
            // Value::Text whose payload is a whitespace-separated
            // list of `<Condition>` self_ids. Empty string clears.
            // Set semantics (de-dup, individual add/remove) are
            // the caller's concern for v1.
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = run.applied_conditions.join(" ");
            run.applied_conditions = if new_val.is_empty() {
                Vec::new()
            } else {
                new_val.split_whitespace().map(|s| s.to_string()).collect()
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        // W0.1 — string-valued character properties. Each stores the
        // raw IDML attribute string (enum strings pass through
        // verbatim — the toggle-group UI never emits an unknown
        // value). The empty string clears the per-run override back
        // to `None` (inherit from the style cascade); the inverse
        // re-applies the captured prior string, which round-trips a
        // prior-`None` back to `None` since `unwrap_or_default()`
        // maps `None ⇒ ""`.
        PropertyPath::CharacterFontFamily => set_run_text_field(run, path, value, |r| &mut r.font),
        PropertyPath::CharacterFontStyle => {
            set_run_text_field(run, path, value, |r| &mut r.font_style)
        }
        PropertyPath::CharacterKerningMethod => {
            set_run_text_field(run, path, value, |r| &mut r.kerning_method)
        }
        PropertyPath::CharacterCase => {
            set_run_text_field(run, path, value, |r| &mut r.capitalization)
        }
        PropertyPath::CharacterPosition => {
            set_run_text_field(run, path, value, |r| &mut r.position)
        }
        PropertyPath::CharacterLanguage => {
            set_run_text_field(run, path, value, |r| &mut r.applied_language)
        }
        PropertyPath::CharacterOtfFeatures => {
            set_run_text_field(run, path, value, |r| &mut r.otf_features)
        }
        // W0.1 — numeric character properties. `Value::Length(None)`
        // clears the per-run override (inherit from the cascade);
        // the captured prior `Option<f32>` round-trips bytewise.
        PropertyPath::CharacterBaselineShift => {
            set_run_length_field(run, path, value, |r| &mut r.baseline_shift)
        }
        PropertyPath::CharacterHorizontalScale => {
            set_run_length_field(run, path, value, |r| &mut r.horizontal_scale)
        }
        PropertyPath::CharacterVerticalScale => {
            set_run_length_field(run, path, value, |r| &mut r.vertical_scale)
        }
        PropertyPath::CharacterSkew => set_run_length_field(run, path, value, |r| &mut r.skew),
        // W0.1 — boolean character properties. `Value::Bool` carries
        // the new toggle; the field is `Option<bool>`. The inverse
        // captures `prev.unwrap_or(false)` — writes over an explicit
        // prior round-trip bytewise; a prior-`None` undoes to
        // `Some(false)` (see the path doc-comments for the
        // documented default-restore limitation).
        PropertyPath::CharacterUnderline => {
            set_run_bool_field(run, path, value, false, |r| &mut r.underline)
        }
        PropertyPath::CharacterStrikethru => {
            set_run_bool_field(run, path, value, false, |r| &mut r.strikethru)
        }
        // Ligatures default ON (OpenType/InDesign): undo of a toggle on a
        // defaulted run must restore the visible ON state (punch-list).
        PropertyPath::CharacterLigatures => {
            set_run_bool_field(run, path, value, true, |r| &mut r.ligatures_on)
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: NodeId::StoryRange {
                story_id: String::new(),
                start: 0,
                end: 0,
            },
            path,
        }),
    }
}
