# `preflight` applies Inter, Open Sans and "Phantom Display", a family no
# machine has (the fixture's missing-font finding). InDesign sets a missing
# family in its substitute, Myriad Pro: the reference embeds
# MyriadPro-Regular. Myriad Pro ships with InDesign
# (Resources/Required/fonts) and is not redistributable, so the gate maps
# the missing family to it from corpus/fonts/local/ (git-ignored) on a
# machine that copied it there; elsewhere the default face stands in.
#
# Inter is registered since ragged text breaks by minimum raggedness
# (ADR 142): set in Inter, the overset story's second line keeps "story"
# whole on the 360 pt measure, as InDesign's Paragraph Composer does.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
if [ -f "$FONTS/local/MyriadPro-Regular.otf" ]; then
    FONT_FLAGS+=(--font-family "Phantom Display=$FONTS/local/MyriadPro-Regular.otf")
fi
