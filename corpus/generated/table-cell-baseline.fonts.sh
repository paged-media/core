# `table-cell-baseline` sets its cells in Open Sans and, for the second em
# box (row 9 and two of the frames below the table), Lora
# (`samples/table_cell_baseline.rs`), both the faces InDesign exported it
# with: the PDF embeds OpenSans-Regular and Lora-Regular.
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Lora=$FONTS/Lora.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
