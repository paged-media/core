# `preflight` applies Inter, Open Sans and "Phantom Display", a family no
# machine has (the fixture's missing-font finding). InDesign sets a missing
# family in its substitute, Myriad Pro: the reference embeds
# MyriadPro-Regular. Myriad Pro ships with InDesign
# (Resources/Required/fonts) and is not redistributable, so the gate reads
# it from corpus/fonts/local/ (git-ignored) on a machine that copied it
# there; elsewhere the default face stands in for the missing family.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
if [ -f "$FONTS/local/MyriadPro-Regular.otf" ]; then
    DEFAULT_FONT="$FONTS/local/MyriadPro-Regular.otf"
fi
