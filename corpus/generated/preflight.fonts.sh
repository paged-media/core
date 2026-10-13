# `preflight` applies Inter, Open Sans and "Phantom Display", a family no
# machine has (the fixture's missing-font finding). InDesign sets a missing
# family in its substitute, Myriad Pro: the reference embeds
# MyriadPro-Regular. Myriad Pro ships with InDesign
# (Resources/Required/fonts) and is not redistributable, so the gate maps
# the missing family to it from corpus/fonts/local/ (git-ignored) on a
# machine that copied it there; elsewhere the default face stands in.
#
# Inter is registered: set in it, the engine's composer once hyphenated
# "sto-ry" on the overset story's second line where InDesign keeps "story"
# whole. The paragraph states no HyphenationZone and the engine took that as
# no zone; InDesign's factory zone is 36 pt (ADR 141), inside which a ragged
# line is not hyphenated.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
if [ -f "$FONTS/local/MyriadPro-Regular.otf" ]; then
    FONT_FLAGS+=(--font-family "Phantom Display=$FONTS/local/MyriadPro-Regular.otf")
fi
