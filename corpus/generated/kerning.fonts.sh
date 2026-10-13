# `kerning` sets its specimen in Inter 20 pt (`samples/kerning.rs`
# SPECIMEN_FONT), the face InDesign exported it with and one whose GPOS
# kerns the specimen's pairs; its labels stay in Open Sans. Without Inter
# the specimen would fall back to a substitute and the gate would measure
# a font swap instead of the kerning.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
