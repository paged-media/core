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

//! The exporter's graphics-state discipline.
//!
//! PDF has no pop-clip: a clip only unwinds via `Q`. The display
//! list interleaves `PushClip`/`PopClip` with blend groups and bare
//! absolute-transform fills, so the mapping is:
//!
//! - every Push/Begin opens EXACTLY ONE `q` and pushes a frame
//!   (PushClip additionally emits its clip path + `W n` right after
//!   the `q`);
//! - the matching Pop/End emits exactly one `Q` and pops;
//! - every LEAF primitive (fill/stroke/glyph/image) wraps its own
//!   `cm` in a private `q … Q`, so object transforms never pollute
//!   a clip frame's CTM and the clip stays active across siblings.
//!
//! The only state living on the PDF stack is the page-flip CTM plus
//! one frame per open Push/Begin. Mismatched pairs are tolerated
//! exactly like the rasterizer: unclosed frames get their `Q`s
//! flushed at page end.

use pdf_writer::Content;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Clip,
    BlendGroup,
    Layer,
}

#[derive(Default)]
pub struct StateStack {
    frames: Vec<FrameKind>,
}

impl StateStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a frame: emits `q`, records the kind. The caller emits
    /// any frame-scoped ops (clip path + `W n`, `gs`) right after.
    pub fn push(&mut self, content: &mut Content, kind: FrameKind) {
        content.save_state();
        self.frames.push(kind);
    }

    /// Close the innermost frame of `kind`. Tolerates mismatches
    /// (no frame open → no-op) like the rasterizer does.
    pub fn pop(&mut self, content: &mut Content, kind: FrameKind) {
        // Well-formed lists close in LIFO order; tolerate the
        // common slight mismatch (a stray Pop) by only popping when
        // the top matches OR any frame of that kind exists.
        if self.frames.last() == Some(&kind) {
            self.frames.pop();
            content.restore_state();
        } else if let Some(pos) = self.frames.iter().rposition(|f| *f == kind) {
            // Out-of-order close: unwind to and including the frame
            // (mirrors save/restore semantics — inner state dies
            // with it, which is what the rasterizer's buffer pops
            // do too).
            let count = self.frames.len() - pos;
            for _ in 0..count {
                self.frames.pop();
                content.restore_state();
            }
        }
    }

    /// Flush any unclosed frames at page end.
    pub fn flush(&mut self, content: &mut Content) {
        while self.frames.pop().is_some() {
            content.restore_state();
        }
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }
}
