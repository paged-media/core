# `split-boundaries` sets every paragraph in Inter (`samples/span_columns.rs` BODY_FONT), the
# face InDesign exported it with. Without registering it the body copy
# would fall back to a substitute and the split-block boundaries gate would measure
# a font swap instead of the split-block boundary rules.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
