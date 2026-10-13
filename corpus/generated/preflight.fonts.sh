# `preflight` applies Inter, Open Sans and "Phantom Display", a family no
# machine has (the fixture's missing-font finding). InDesign sets a missing
# family in its substitute, Myriad Pro: the reference embeds
# MyriadPro-Regular. Myriad Pro ships with InDesign
# (Resources/Required/fonts) and is not redistributable, so the gate maps
# the missing family to it from corpus/fonts/local/ (git-ignored) on a
# machine that copied it there; elsewhere the default face stands in.
#
# Inter is deliberately NOT registered here yet: set in Inter, the
# engine's composer hyphenates "sto-ry" on the overset story's second line
# where InDesign's Paragraph Composer fits "story" on the full 360 pt
# measure (docs/design/text-gate-triage-2026-10.md, R-20 / R-23). Until
# that lands, the default face keeps the text gate on what it has passed.
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
if [ -f "$FONTS/local/MyriadPro-Regular.otf" ]; then
    FONT_FLAGS+=(--font-family "Phantom Display=$FONTS/local/MyriadPro-Regular.otf")
fi
