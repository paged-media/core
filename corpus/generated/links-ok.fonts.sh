# `links-ok` sets its text in Inter, the face InDesign exported it with (its
# reference embeds Inter-Regular). Without it the gate rendered the
# fallback serif and measured a font swap: the text gate found lines a
# different face fits in a frame InDesign's does not.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
