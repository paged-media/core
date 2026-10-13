# `annual-base` applies seven families, all installed for InDesign from
# corpus/fonts (its reference embeds the five its printed pages use) and
# registered here from the same files, so the gate measures the layout and
# not a font swap.
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
    --font-family "Source Serif 4=$FONTS/SourceSerif4.ttf"
    --font-family "JetBrains Mono=$FONTS/JetBrainsMono-VF.ttf"
    --font-family "JetBrains Mono/Italic=$FONTS/JetBrainsMono-Italic-VF.ttf"
    --font-family "Space Grotesk=$FONTS/SpaceGrotesk-VF.ttf"
    --font-family "Noto Sans JP=$FONTS/NotoSansJP-VF.ttf"
    --font-family "EB Garamond=$FONTS/EBGaramond-VF.ttf"
    --font-family "EB Garamond/Italic=$FONTS/EBGaramond-Italic-VF.ttf"
    --font-family "Fraunces=$FONTS/Fraunces-VF.ttf"
    --font-family "Fraunces/Italic=$FONTS/Fraunces-Italic-VF.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
