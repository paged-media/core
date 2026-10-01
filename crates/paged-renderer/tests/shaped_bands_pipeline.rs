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

//! Lines in SHAPED text frames, over the generated `shaped-bands.idml`,
//! line by line against where InDesign 20.0.1 put every line of the
//! same file (2026-10-01; the fixture is also in the fidelity gate with
//! InDesign's PDF export as reference).
//!
//! The rules InDesign follows (`build_perline_wrap_widths` implements
//! them):
//!
//! - a line's band is the narrowest chord of the outline — eroded by the
//!   inset plus the stroke's share — over its slug, from the PREVIOUS
//!   line's baseline down to its own, whichever paragraph that line
//!   belongs to; the frame's first line reaches up by its first-baseline
//!   distance (the ascent for `AscentOffset`, the leading for
//!   `LeadingOffset`);
//! - the band's ends are floored onto a whole-point grid that starts at
//!   the text area's left edge, and an end exactly on the grid stays;
//! - the first baseline walks down whole points until the first word
//!   fits;
//! - a line a hole splits is ONE line: left-aligned, each part sits flush
//!   left; right-aligned, flush right; centred, the parts close in on the
//!   hole (the first flush right, the last flush left);
//! - ragged lines break by minimum raggedness — the least sum of each
//!   line's squared slack, the parts of a split line priced apart, the
//!   overset lines past the first free (`paged_text` `ragged.rs`), which
//!   is what makes a triangle set `delta | echo foxtrot` where filling the
//!   narrow line first sets `delta echo | foxtrot golf`.

use paged_gen::samples::shaped_bands::{
    body_story_id, cases, justification, long_text, paragraph_text, Body, PARAGRAPHS,
};
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

/// Every line InDesign set, per case: (case, text, ink start, ink end,
/// baseline), page points — the first and last visible character's
/// `horizontalOffset` / `endHorizontalOffset` and the line's `baseline`,
/// read from InDesign's DOM after opening the generated IDML. A line a
/// hole splits is one InDesign line; its ink runs from the first part's
/// start to the last part's end.
const INDESIGN: &[(usize, &str, f32, f32, f32)] = &[
    (0, "alpha", 145.000, 170.688, 70.688),
    (0, "bravo charlie", 114.000, 175.426, 82.688),
    (0, "delta echo foxtrot golf", 96.000, 200.570, 94.688),
    (
        0,
        "hotel india juliet kilo lima mike",
        84.000,
        223.990,
        106.688,
    ),
    (0, "november oscar papa quebec", 75.000, 216.548, 118.688),
    (
        0,
        "romeo sierra tango uniform victor",
        68.000,
        226.252,
        130.688,
    ),
    (
        0,
        "whiskey xray yankee zulu alpha bravo",
        64.000,
        242.945,
        142.688,
    ),
    (
        0,
        "charlie delta echo foxtrot golf hotel india",
        61.000,
        251.669,
        154.688,
    ),
    (
        0,
        "juliet kilo lima mike november oscar papa",
        60.000,
        254.556,
        166.688,
    ),
    (
        0,
        "quebec romeo sierra tango uniform victor",
        61.000,
        257.597,
        178.688,
    ),
    (
        0,
        "whiskey xray yankee zulu alpha bravo",
        64.000,
        242.945,
        190.688,
    ),
    (
        0,
        "charlie delta echo foxtrot golf hotel",
        69.000,
        234.366,
        202.688,
    ),
    (
        0,
        "india juliet kilo lima mike november",
        76.000,
        240.097,
        214.688,
    ),
    (0, "oscar papa quebec romeo", 85.000, 209.287, 226.688),
    (0, "sierra tango uniform", 98.000, 193.757, 238.688),
    (0, "victor whiskey", 117.000, 184.817, 250.688),
    (1, "alpha", 408.312, 434.000, 70.688),
    (1, "bravo charlie", 403.574, 465.000, 82.688),
    (1, "delta echo foxtrot golf", 378.430, 483.000, 94.688),
    (
        1,
        "hotel india juliet kilo lima mike",
        355.010,
        495.000,
        106.688,
    ),
    (1, "november oscar papa quebec", 362.452, 504.000, 118.688),
    (
        1,
        "romeo sierra tango uniform victor",
        352.748,
        511.000,
        130.688,
    ),
    (
        1,
        "whiskey xray yankee zulu alpha bravo",
        336.055,
        515.000,
        142.688,
    ),
    (
        1,
        "charlie delta echo foxtrot golf hotel india",
        327.331,
        518.000,
        154.688,
    ),
    (
        1,
        "juliet kilo lima mike november oscar papa",
        324.444,
        519.000,
        166.688,
    ),
    (
        1,
        "quebec romeo sierra tango uniform victor",
        321.403,
        518.000,
        178.688,
    ),
    (
        1,
        "whiskey xray yankee zulu alpha bravo",
        336.055,
        515.000,
        190.688,
    ),
    (
        1,
        "charlie delta echo foxtrot golf hotel",
        344.634,
        510.000,
        202.688,
    ),
    (
        1,
        "india juliet kilo lima mike november",
        338.903,
        503.000,
        214.688,
    ),
    (1, "oscar papa quebec romeo", 369.713, 494.000, 226.688),
    (1, "sierra tango uniform", 385.243, 481.000, 238.688),
    (1, "victor whiskey", 394.183, 462.000, 250.688),
    (2, "alpha", 146.000, 171.688, 339.000),
    (2, "bravo charlie", 112.000, 173.426, 351.000),
    (2, "delta echo foxtrot golf", 96.000, 200.570, 363.000),
    (
        2,
        "hotel india juliet kilo lima mike",
        85.000,
        224.990,
        375.000,
    ),
    (2, "november oscar papa quebec", 77.000, 218.548, 387.000),
    (
        2,
        "romeo sierra tango uniform victor",
        71.000,
        229.252,
        399.000,
    ),
    (
        2,
        "whiskey xray yankee zulu alpha bravo",
        68.000,
        246.945,
        411.000,
    ),
    (
        2,
        "charlie delta echo foxtrot golf hotel",
        66.000,
        231.366,
        423.000,
    ),
    (
        2,
        "india juliet kilo lima mike november",
        67.000,
        231.097,
        435.000,
    ),
    (
        2,
        "oscar papa quebec romeo sierra",
        69.000,
        222.931,
        447.000,
    ),
    (2, "tango uniform victor whiskey", 74.000, 210.743, 459.000),
    (2, "xray yankee zulu alpha bravo", 81.000, 218.817, 471.000),
    (2, "charlie delta echo foxtrot", 90.000, 208.154, 483.000),
    (2, "golf hotel india juliet", 103.000, 197.795, 495.000),
    (2, "kilo lima mike", 124.000, 187.447, 507.000),
    (3, "alpha", 408.312, 434.000, 335.000),
    (3, "bravo charlie delta", 380.305, 468.000, 347.000),
    (
        3,
        "echo foxtrot golf hotel india",
        355.249,
        485.000,
        359.000,
    ),
    (
        3,
        "juliet kilo lima mike november",
        357.206,
        496.000,
        371.000,
    ),
    (
        3,
        "oscar papa quebec romeo sierra",
        350.069,
        504.000,
        383.000,
    ),
    (
        3,
        "tango uniform victor whiskey xray",
        350.132,
        510.000,
        395.000,
    ),
    (
        3,
        "yankee zulu alpha bravo charlie delta",
        338.390,
        514.000,
        407.000,
    ),
    (
        3,
        "echo foxtrot golf hotel india juliet kilo",
        343.016,
        517.000,
        419.000,
    ),
    (
        3,
        "lima mike november oscar papa quebec",
        328.333,
        517.000,
        431.000,
    ),
    (
        3,
        "romeo sierra tango uniform victor",
        356.748,
        515.000,
        443.000,
    ),
    (
        3,
        "whiskey xray yankee zulu alpha",
        361.645,
        511.000,
        455.000,
    ),
    (
        3,
        "bravo charlie delta echo foxtrot",
        357.256,
        505.000,
        467.000,
    ),
    (
        3,
        "golf hotel india juliet kilo lima",
        362.033,
        498.000,
        479.000,
    ),
    (3, "mike november oscar papa", 358.709, 487.000, 491.000),
    (3, "quebec romeo sierra", 373.831, 472.000, 503.000),
    (3, "tango", 417.027, 444.000, 515.000),
    (4, "alpha", 156.000, 181.688, 613.688),
    (4, "bravo", 151.000, 177.777, 625.688),
    (4, "charlie", 144.000, 175.836, 637.688),
    (4, "delta", 138.000, 161.457, 649.688),
    (4, "echo foxtrot", 131.000, 188.236, 661.688),
    (4, "golf hotel india", 125.000, 194.702, 673.688),
    (4, "juliet kilo lima mike", 118.000, 206.540, 685.688),
    (4, "november oscar papa", 111.000, 214.203, 697.688),
    (4, "quebec romeo sierra tango", 105.000, 232.954, 709.688),
    (4, "uniform victor whiskey xray", 98.000, 228.083, 721.688),
    (
        4,
        "yankee zulu alpha bravo charlie",
        92.000,
        241.341,
        733.688,
    ),
    (
        4,
        "delta echo foxtrot golf hotel india",
        85.000,
        241.021,
        745.688,
    ),
    (
        4,
        "juliet kilo lima mike november oscar",
        78.000,
        246.071,
        757.688,
    ),
    (
        4,
        "papa quebec romeo sierra tango uniform",
        72.000,
        265.579,
        769.688,
    ),
    (5, "alpha", 417.312, 443.000, 621.688),
    (5, "bravo", 421.223, 448.000, 633.688),
    (5, "charlie", 422.164, 454.000, 645.688),
    (5, "delta", 437.543, 461.000, 657.688),
    (5, "echo foxtrot", 410.764, 468.000, 669.688),
    (5, "golf hotel india", 404.298, 474.000, 681.688),
    (5, "juliet kilo lima mike", 392.460, 481.000, 693.688),
    (5, "november oscar papa", 383.797, 487.000, 705.688),
    (5, "quebec romeo sierra tango", 366.046, 494.000, 717.688),
    (5, "uniform victor whiskey xray", 370.917, 501.000, 729.688),
    (
        5,
        "yankee zulu alpha bravo charlie",
        357.659,
        507.000,
        741.688,
    ),
    (
        5,
        "delta echo foxtrot golf hotel india",
        357.979,
        514.000,
        753.688,
    ),
    (
        5,
        "juliet kilo lima mike november oscar",
        351.929,
        520.000,
        765.688,
    ),
    (
        6,
        "alpha bravo charlie delta echo foxtrot golf",
        60.000,
        257.310,
        69.688,
    ),
    (
        6,
        "hotel india juliet kilo lima mike november oscar",
        60.000,
        279.521,
        81.688,
    ),
    (
        6,
        "papa quebec romeo sierra tango uniform",
        60.000,
        253.579,
        93.688,
    ),
    (
        6,
        "victor whiskey xray yankee zulu alpha bravo",
        60.000,
        268.447,
        105.688,
    ),
    (
        6,
        "charlie delta echo foxtrot golf hotel india juliet",
        60.000,
        275.762,
        117.688,
    ),
    (6, "kilo lima mike november", 60.000, 257.441, 129.688),
    (6, "oscar papa quebec romeo", 60.000, 278.525, 141.688),
    (6, "sierra tango uniform victor", 60.000, 275.830, 153.688),
    (6, "whiskey xray yankee zulu", 60.000, 266.602, 165.688),
    (6, "alpha bravo charlie delta", 60.000, 268.105, 177.688),
    (6, "echo foxtrot golf hotel", 60.000, 254.399, 189.688),
    (6, "india juliet kilo lima mike", 60.000, 273.447, 201.688),
    (
        6,
        "november oscar papa quebec romeo sierra",
        60.000,
        264.185,
        213.688,
    ),
    (
        6,
        "tango uniform victor whiskey xray yankee zulu",
        60.000,
        279.282,
        225.688,
    ),
    (
        6,
        "alpha bravo charlie delta echo foxtrot golf",
        60.000,
        257.310,
        237.688,
    ),
    (
        6,
        "hotel india juliet kilo lima mike november oscar",
        60.000,
        279.521,
        249.688,
    ),
    (
        7,
        "alpha bravo charlie delta echo foxtrot golf",
        342.690,
        540.000,
        69.688,
    ),
    (
        7,
        "hotel india juliet kilo lima mike november oscar",
        320.479,
        540.000,
        81.688,
    ),
    (
        7,
        "papa quebec romeo sierra tango uniform",
        346.421,
        540.000,
        93.688,
    ),
    (
        7,
        "victor whiskey xray yankee zulu alpha bravo",
        331.553,
        540.000,
        105.688,
    ),
    (
        7,
        "charlie delta echo foxtrot golf hotel india juliet",
        324.238,
        540.000,
        117.688,
    ),
    (7, "kilo lima mike november", 326.553, 540.000, 129.688),
    (7, "oscar papa quebec romeo", 337.051, 540.000, 141.688),
    (7, "sierra tango uniform victor", 333.384, 540.000, 153.688),
    (7, "whiskey xray yankee zulu", 328.560, 540.000, 165.688),
    (7, "alpha bravo charlie delta", 334.722, 540.000, 177.688),
    (7, "echo foxtrot golf hotel", 332.764, 540.000, 189.688),
    (7, "india juliet kilo lima mike", 342.417, 540.000, 201.688),
    (
        7,
        "november oscar papa quebec romeo sierra",
        335.815,
        540.000,
        213.688,
    ),
    (
        7,
        "tango uniform victor whiskey xray yankee zulu",
        320.718,
        540.000,
        225.688,
    ),
    (
        7,
        "alpha bravo charlie delta echo foxtrot golf",
        342.690,
        540.000,
        237.688,
    ),
    (
        7,
        "hotel india juliet kilo lima mike november oscar",
        320.479,
        540.000,
        249.688,
    ),
    (
        8,
        "alpha bravo charlie delta echo foxtrot golf",
        71.345,
        268.655,
        329.688,
    ),
    (
        8,
        "hotel india juliet kilo lima mike november oscar",
        60.239,
        279.761,
        341.688,
    ),
    (
        8,
        "papa quebec romeo sierra tango uniform",
        73.210,
        266.790,
        353.688,
    ),
    (
        8,
        "victor whiskey xray yankee zulu alpha bravo",
        65.776,
        274.224,
        365.688,
    ),
    (
        8,
        "charlie delta echo foxtrot golf hotel india juliet",
        62.119,
        277.881,
        377.688,
    ),
    (8, "kilo lima mike november", 66.553, 257.441, 389.688),
    (8, "oscar papa quebec romeo", 77.051, 278.525, 401.688),
    (8, "sierra tango uniform victor", 73.384, 275.830, 413.688),
    (8, "whiskey xray yankee zulu", 68.560, 266.602, 425.688),
    (8, "alpha bravo charlie delta", 74.722, 268.105, 437.688),
    (8, "echo foxtrot golf hotel", 72.764, 254.399, 449.688),
    (8, "india juliet kilo lima mike", 82.417, 273.447, 461.688),
    (
        8,
        "november oscar papa quebec romeo sierra",
        67.908,
        272.092,
        473.688,
    ),
    (
        8,
        "tango uniform victor whiskey xray yankee zulu",
        60.359,
        279.641,
        485.688,
    ),
    (
        8,
        "alpha bravo charlie delta echo foxtrot golf",
        71.345,
        268.655,
        497.688,
    ),
    (
        8,
        "hotel india juliet kilo lima mike november oscar",
        60.239,
        279.761,
        509.688,
    ),
    (
        9,
        "alpha bravo charlie delta echo foxtrot golf",
        320.000,
        540.000,
        329.688,
    ),
    (
        9,
        "hotel india juliet kilo lima mike november oscar",
        320.000,
        540.000,
        341.688,
    ),
    (
        9,
        "papa quebec romeo sierra tango uniform",
        320.000,
        540.000,
        353.688,
    ),
    (
        9,
        "victor whiskey xray yankee zulu alpha bravo",
        320.000,
        540.000,
        365.688,
    ),
    (
        9,
        "charlie delta echo foxtrot golf hotel india juliet",
        320.000,
        540.000,
        377.688,
    ),
    (9, "kilo lima mike november", 320.000, 540.000, 389.688),
    (9, "oscar papa quebec romeo", 320.000, 540.000, 401.688),
    (9, "sierra tango uniform victor", 320.000, 540.000, 413.688),
    (9, "whiskey xray yankee zulu", 320.000, 540.000, 425.688),
    (9, "alpha bravo charlie delta", 320.000, 540.000, 437.688),
    (9, "echo foxtrot golf hotel", 320.000, 540.000, 449.688),
    (9, "india juliet kilo lima mike", 320.000, 540.000, 461.688),
    (
        9,
        "november oscar papa quebec romeo sierra",
        320.000,
        540.000,
        473.688,
    ),
    (
        9,
        "tango uniform victor whiskey xray yankee zulu",
        320.000,
        540.000,
        485.688,
    ),
    (
        9,
        "alpha bravo charlie delta echo foxtrot golf",
        320.000,
        540.000,
        497.688,
    ),
    (
        9,
        "hotel india juliet kilo lima mike november oscar",
        320.000,
        540.000,
        509.688,
    ),
    (
        10,
        "alpha bravo charlie delta echo foxtrot golf",
        71.220,
        268.530,
        594.062,
    ),
    (
        10,
        "hotel india juliet kilo lima mike november",
        74.753,
        264.997,
        606.062,
    ),
    (
        10,
        "oscar papa quebec romeo sierra tango",
        78.017,
        261.733,
        618.062,
    ),
    (
        10,
        "uniform victor whiskey xray yankee zulu",
        75.126,
        264.624,
        630.062,
    ),
    (10, "alpha bravo charlie", 70.097, 246.211, 642.062),
    (10, "delta echo foxtrot golf", 75.653, 266.411, 654.062),
    (10, "hotel india juliet kilo", 76.737, 255.796, 666.062),
    (10, "lima mike november", 81.068, 261.816, 678.062),
    (10, "oscar papa quebec", 72.426, 249.907, 690.062),
    (10, "romeo sierra tango", 95.194, 270.991, 702.062),
    (10, "uniform victor", 89.047, 241.064, 714.062),
    (10, "whiskey xray", 87.060, 234.688, 726.062),
    (
        10,
        "yankee zulu alpha bravo charlie delta echo",
        68.938,
        270.812,
        738.062,
    ),
    (
        10,
        "foxtrot golf hotel india juliet kilo lima mike",
        72.456,
        267.294,
        750.062,
    ),
    (
        10,
        "november oscar papa quebec romeo sierra",
        67.783,
        271.967,
        762.062,
    ),
    (
        10,
        "tango uniform victor whiskey xray yankee",
        71.525,
        268.225,
        774.062,
    ),
    (
        11,
        "alpha bravo charlie delta echo foxtrot golf",
        323.000,
        520.310,
        592.688,
    ),
    (
        11,
        "hotel india juliet kilo lima mike november",
        323.000,
        513.244,
        604.688,
    ),
    (
        11,
        "oscar papa quebec romeo sierra tango",
        323.000,
        506.716,
        616.688,
    ),
    (
        11,
        "uniform victor whiskey xray yankee zulu",
        323.000,
        512.497,
        628.688,
    ),
    (11, "alpha bravo charlie delta", 323.000, 531.105, 640.688),
    (11, "echo foxtrot golf hotel", 323.000, 517.399, 652.688),
    (11, "india juliet kilo lima mike", 323.000, 536.447, 664.688),
    (11, "november oscar papa", 323.000, 525.949, 676.688),
    (11, "quebec romeo", 323.000, 503.181, 688.688),
    (11, "sierra tango uniform", 323.000, 509.328, 700.688),
    (11, "victor whiskey xray", 323.000, 534.440, 712.688),
    (11, "yankee zulu alpha bravo", 323.000, 528.278, 724.688),
    (
        11,
        "charlie delta echo foxtrot golf hotel india",
        323.000,
        513.669,
        736.688,
    ),
    (
        11,
        "juliet kilo lima mike november oscar papa",
        323.000,
        517.556,
        748.688,
    ),
    (
        11,
        "quebec romeo sierra tango uniform victor",
        323.000,
        519.597,
        760.688,
    ),
    (
        11,
        "whiskey xray yankee zulu alpha bravo charlie",
        323.000,
        536.594,
        772.688,
    ),
    (12, "1. alpha bravo charlie", 120.000, 219.688, 69.688),
    (12, "2. hotel india juliet kilo", 175.337, 280.000, 81.688),
    (
        12,
        "3. oscar papa quebec romeo sierra",
        98.000,
        263.576,
        93.688,
    ),
    (
        12,
        "4. victor whiskey xray yankee zulu alpha",
        89.326,
        280.000,
        105.688,
    ),
    (12, "5. charlie delta echo", 74.000, 169.728, 117.688),
    (12, "6. juliet kilo lima mike", 179.902, 280.000, 129.688),
    (
        12,
        "7. quebec romeo sierra tango uniform",
        60.000,
        237.197,
        141.688,
    ),
    (
        12,
        "8. xray yankee zulu alpha bravo charlie",
        95.879,
        280.000,
        153.688,
    ),
    (12, "9. echo foxtrot golf", 60.000, 149.941, 165.688),
    (
        12,
        "10. lima mike november oscar",
        140.347,
        280.000,
        177.688,
    ),
    (
        12,
        "11. sierra tango uniform victor whiskey",
        60.000,
        240.215,
        189.688,
    ),
    (
        12,
        "12. zulu alpha bravo charlie delta echo",
        97.097,
        278.000,
        201.688,
    ),
    (13, "1. alpha bravo charlie", 381.000, 480.688, 75.000),
    (13, "2. hotel india juliet kilo", 432.337, 537.000, 87.000),
    (
        13,
        "3. oscar papa quebec romeo sierra",
        357.000,
        522.576,
        99.000,
    ),
    (
        13,
        "4. victor whiskey xray yankee zulu alpha",
        346.326,
        537.000,
        111.000,
    ),
    (13, "5. charlie delta echo", 333.000, 428.728, 123.000),
    (13, "6. juliet kilo lima mike", 436.902, 537.000, 135.000),
    (
        13,
        "7. quebec romeo sierra tango uniform",
        323.000,
        500.197,
        147.000,
    ),
    (
        13,
        "8. xray yankee zulu alpha bravo charlie",
        352.879,
        537.000,
        159.000,
    ),
    (13, "9. echo foxtrot golf", 323.000, 412.941, 171.000),
    (
        13,
        "10. lima mike november oscar",
        397.347,
        537.000,
        183.000,
    ),
    (
        13,
        "11. sierra tango uniform victor whiskey",
        323.000,
        503.215,
        195.000,
    ),
    (
        13,
        "12. zulu alpha bravo charlie delta echo",
        347.097,
        528.000,
        207.000,
    ),
    (
        14,
        "alpha bravo charlie delta echo foxtrot golf",
        60.000,
        257.310,
        329.688,
    ),
    (
        14,
        "hotel india juliet kilo lima mike november oscar",
        60.000,
        279.521,
        341.688,
    ),
    (
        14,
        "papa quebec romeo sierra tango uniform",
        60.000,
        253.579,
        353.688,
    ),
    (
        14,
        "victor whiskey xray yankee zulu alpha bravo",
        60.000,
        268.447,
        365.688,
    ),
    (14, "charlie delta echo foxtrot", 60.000, 178.154, 377.688),
    (14, "golf hotel india juliet kilo", 60.000, 173.936, 389.688),
    (14, "lima mike november oscar", 60.000, 183.838, 401.688),
    (14, "papa quebec romeo sierra", 60.000, 184.653, 413.688),
    (14, "tango uniform victor", 60.000, 155.615, 425.688),
    (14, "whiskey xray yankee zulu", 60.000, 180.854, 437.688),
    (14, "alpha bravo charlie delta", 60.000, 176.196, 449.688),
    (
        14,
        "echo foxtrot golf hotel india juliet kilo lima",
        60.000,
        256.016,
        461.688,
    ),
    (
        14,
        "mike november oscar papa quebec romeo",
        60.000,
        259.629,
        473.688,
    ),
    (
        14,
        "sierra tango uniform victor whiskey xray",
        60.000,
        249.512,
        485.688,
    ),
    (
        14,
        "yankee zulu alpha bravo charlie delta echo",
        60.000,
        261.875,
        497.688,
    ),
    (
        14,
        "foxtrot golf hotel india juliet kilo lima mike",
        60.000,
        254.839,
        509.688,
    ),
    (
        15,
        "alpha bravo charlie delta echo foxtrot golf",
        342.190,
        539.500,
        330.188,
    ),
    (
        15,
        "hotel india juliet kilo lima mike november",
        349.256,
        539.500,
        342.188,
    ),
    (
        15,
        "oscar papa quebec romeo sierra tango",
        355.784,
        539.500,
        354.188,
    ),
    (
        15,
        "uniform victor whiskey xray yankee zulu alpha",
        321.502,
        539.500,
        366.188,
    ),
    (15, "bravo charlie delta echo", 335.540, 449.500, 378.188),
    (
        15,
        "foxtrot golf hotel india juliet",
        320.921,
        449.500,
        390.188,
    ),
    (15, "kilo lima mike november", 335.799, 449.500, 402.188),
    (15, "oscar papa quebec romeo", 325.213, 449.500, 414.188),
    (15, "sierra tango uniform victor", 324.241, 449.500, 426.188),
    (15, "whiskey xray yankee zulu", 328.646, 449.500, 438.188),
    (15, "alpha bravo charlie delta", 333.304, 449.500, 450.188),
    (
        15,
        "echo foxtrot golf hotel india juliet kilo lima",
        343.484,
        539.500,
        462.188,
    ),
    (
        15,
        "mike november oscar papa quebec romeo",
        339.871,
        539.500,
        474.188,
    ),
    (
        15,
        "sierra tango uniform victor whiskey xray",
        349.988,
        539.500,
        486.188,
    ),
    (
        15,
        "yankee zulu alpha bravo charlie delta echo",
        337.625,
        539.500,
        498.188,
    ),
    (
        15,
        "foxtrot golf hotel india juliet kilo lima mike",
        344.661,
        539.500,
        510.188,
    ),
    (16, "alpha", 90.000, 115.688, 594.688),
    (16, "bravo", 81.000, 107.777, 606.688),
    (16, "charlie delta", 74.000, 132.105, 618.688),
    (16, "echo foxtrot", 69.000, 126.236, 630.688),
    (16, "golf hotel india", 65.000, 134.702, 642.688),
    (16, "juliet kilo lima", 63.000, 126.452, 654.688),
    (16, "mike november", 61.000, 133.529, 666.688),
    (16, "oscar papa", 60.000, 112.949, 678.688),
    (16, "quebec romeo", 60.000, 128.525, 690.688),
    (16, "sierra tango", 61.000, 117.616, 702.688),
    (16, "uniform victor", 62.000, 127.830, 714.688),
    (16, "whiskey xray", 65.000, 126.440, 726.688),
    (16, "yankee zulu", 68.000, 124.602, 738.688),
    (16, "alpha bravo", 73.000, 128.278, 750.688),
    (16, "charlie", 79.000, 110.836, 762.688),
    (16, "delta", 90.000, 113.457, 774.688),
    (17, "1.", 358.000, 364.948, 590.688),
    (17, "alpha", 344.000, 369.688, 602.688),
    (17, "bravo", 336.000, 362.777, 614.688),
    (17, "charlie", 330.000, 361.836, 626.688),
    (17, "2. hotel india", 342.570, 403.000, 638.688),
    (17, "juliet kilo", 363.579, 405.000, 650.688),
    (17, "3. oscar papa", 321.000, 385.595, 662.688),
    (17, "quebec romeo", 320.000, 388.525, 674.688),
    (17, "sierra", 320.000, 346.831, 686.688),
    (17, "4. victor whiskey", 329.366, 409.000, 698.688),
    (17, "xray yankee zulu", 327.273, 407.000, 710.688),
    (17, "alpha", 379.312, 405.000, 722.688),
    (17, "5. charlie delta", 327.000, 396.463, 734.688),
    (17, "echo", 331.000, 354.452, 746.688),
    (17, "6. juliet kilo", 339.021, 392.000, 758.688),
    (17, "lima", 363.781, 383.000, 770.688),
];

/// Cases the engine cannot match yet, with the reason. Everything else
/// must land within [`TOLERANCE`].
const KNOWN: &[(usize, &str)] = &[
    // A justified row a hole splits: InDesign letterspaces a lone word
    // to fill its part (`november` across [470, 540]) and breaks the rows
    // differently — justified text is still Knuth–Plass here, not
    // minimum raggedness, and its twin-row stretch bump is a guess.
    (9, "justified rows split by a hole"),
];

/// Single lines the engine cannot match yet, by (case, InDesign's text).
const KNOWN_LINES: &[(usize, &str, &str)] = &[
    // A 4 pt stroke on the 200 pt circle erodes it to radius 98. One
    // point under it InDesign's band reaches 114 (the eroded κ-curve:
    // 114.02), but 47 pt below the centre it stops at 185 where the same
    // curve reaches 186.01 and a true circle 185.99. Whatever InDesign
    // offsets a curve with is neither; every other line of the case, and
    // every line of the unstroked inset circle, lands.
    (
        3,
        "bravo charlie delta echo foxtrot",
        "a stroke-eroded curve, 1 pt",
    ),
];

const TOLERANCE: f32 = 0.05;

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

/// One visible row: a line, or the parts of a line a hole splits.
struct Row {
    text: String,
    left: f32,
    right: f32,
    baseline: f32,
    paragraph: usize,
}

fn rows(built: &pipeline::BuiltDocument, i: usize, body: Body) -> Vec<Row> {
    let text_of = |p: usize| match body {
        Body::Long(_) => long_text(),
        Body::Paragraphs => paragraph_text(p),
    };
    let mut rows: Vec<Row> = Vec::new();
    for line in built.story_layout(&body_story_id(i as u32)) {
        let p = line.paragraph_idx as usize;
        let text = text_of(p);
        let ink: Vec<_> = line
            .clusters
            .iter()
            .filter(|c| {
                text[c.byte as usize..]
                    .chars()
                    .next()
                    .is_some_and(|ch| !ch.is_whitespace())
            })
            .collect();
        let (Some(first), Some(last)) = (ink.first(), ink.last()) else {
            continue;
        };
        let left = ink.iter().map(|c| c.x_pt).fold(f32::INFINITY, f32::min);
        let right = ink
            .iter()
            .map(|c| c.x_pt + c.advance_pt)
            .fold(f32::NEG_INFINITY, f32::max);
        let words = text[first.byte as usize..]
            .split_at(
                (last.byte - first.byte) as usize
                    + text[last.byte as usize..]
                        .chars()
                        .next()
                        .map_or(0, char::len_utf8),
            )
            .0
            .to_string();
        match rows.last_mut() {
            Some(r) if (r.baseline - line.baseline_y_pt).abs() < 0.01 => {
                r.text = format!("{} {words}", r.text);
                r.left = r.left.min(left);
                r.right = r.right.max(right);
            }
            _ => rows.push(Row {
                text: words,
                left,
                right,
                baseline: line.baseline_y_pt,
                paragraph: p,
            }),
        }
    }
    rows
}

#[test]
fn every_line_lands_where_indesign_puts_it() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::shaped_bands::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    // Registered under its family name, as the corpus gate does: a face
    // handed over only as the fallback font is a substitute, and a
    // substitute's ascent is not the face's own.
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, inter_font());
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    let mut report = Vec::new();
    let mut failures = 0;
    let mut checked = 0;
    for (i, case) in cases().iter().enumerate() {
        let known = KNOWN.iter().find(|k| k.0 == i).map(|k| k.1);
        let want: Vec<_> = INDESIGN.iter().filter(|r| r.0 == i).collect();
        let got = rows(&built, i, case.copy);
        report.push(format!(
            "── {i:2} {} ({} lines, InDesign {}){}",
            case.name,
            got.len(),
            want.len(),
            known.map(|r| format!("  [known: {r}]")).unwrap_or_default()
        ));
        let mut bad = got.len() != want.len();
        for (k, w) in want.iter().enumerate() {
            let Some(row) = got.get(k) else {
                report.push(format!("   ✗ missing          {}", w.1));
                continue;
            };
            let j = justification(case, row.paragraph.min(PARAGRAPHS - 1));
            // The edge the band puts a line against: the start of a
            // left-aligned line, the end of a right-aligned one, both
            // ends of a centred or justified one.
            let check_left = j != "RightAlign";
            let check_right = j != "LeftAlign" && !(j == "LeftJustified" && k + 1 == want.len());
            let ok_text = row.text == w.1;
            let ok_left = !check_left || (row.left - w.2).abs() <= TOLERANCE;
            let ok_right = !check_right || (row.right - w.3).abs() <= TOLERANCE;
            let ok_y = (row.baseline - w.4).abs() <= TOLERANCE;
            let ok = ok_text && ok_left && ok_right && ok_y;
            let line_known = KNOWN_LINES.iter().any(|k| k.0 == i && k.1 == w.1);
            if line_known && ok {
                report.push(format!(
                    "   ^ {} matches now: drop it from KNOWN_LINES",
                    w.1
                ));
                failures += 1;
            }
            bad |= !ok && !line_known;
            report.push(format!(
                "   {} x {:8.3}..{:8.3} y {:8.3} | id {:8.3}..{:8.3} y {:8.3}  {}{}",
                if ok { " " } else { "✗" },
                row.left,
                row.right,
                row.baseline,
                w.2,
                w.3,
                w.4,
                w.1,
                if ok_text {
                    String::new()
                } else {
                    format!("   ENGINE: {}", row.text)
                }
            ));
        }
        match (bad, known) {
            (true, None) => failures += 1,
            (false, Some(_)) => {
                report.push("   ^ matches now: drop it from KNOWN".to_string());
                failures += 1;
            }
            _ => {}
        }
        if known.is_none() {
            checked += 1;
        }
    }
    if std::env::var_os("SHAPED_BANDS_REPORT").is_some() {
        eprintln!("{}", report.join("\n"));
    }
    assert!(checked >= 12, "only {checked} cases checked");
    assert_eq!(failures, 0, "\n{}", report.join("\n"));
}
