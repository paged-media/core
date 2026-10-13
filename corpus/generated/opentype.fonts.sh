# `opentype` sets its cases in Inter, EB Garamond (roman and italic) and
# Roboto (`samples/opentype.rs`), its labels in Open Sans — the faces
# InDesign exported it with (`pdffonts opentype.pdf`: all embedded, none
# substituted). The features under test live in these exact files: a
# substitute would measure a font swap, not the OpenType features.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "EB Garamond=$FONTS/EBGaramond-VF.ttf"
    --font-family "EB Garamond/Italic=$FONTS/EBGaramond-Italic-VF.ttf"
    --font-family "Roboto=$FONTS/Roboto-Regular.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
