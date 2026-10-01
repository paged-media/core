# `reflow` sets its body copy in Inter. Its reference PDF is InDesign's
# export AFTER Smart Text Reflow grew the document (reflow-probe.sh), so
# the render must grow too: `--grow-story` gives the body story the grow
# rule IDML cannot carry (InDesign does not reflow on open). These are
# per-fixture inspect flags; the array name is the harness's.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
    --grow-story "u72f44a"
)
