# `keeps-reflow` sets every paragraph in Inter (`samples/keeps_reflow.rs`).
# Its reference PDF is InDesign's export AFTER the story was reflowed over
# generated pages (reflow-probe.sh, PAGED_REFLOW_EDIT=thread), so the
# render must grow too: `--grow-story` gives the body story the grow rule
# IDML cannot carry. These are per-fixture inspect flags; the array name is
# the harness's.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
    --grow-story "u5e31de"
)
