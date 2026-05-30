//! Per-resource XML builders. Each module takes a builder shape and
//! produces the XML bytes the package writer stitches together.

pub mod designmap;
pub mod master;
pub mod page_item;
pub mod resources;
pub mod spread;
pub mod story;
pub mod xml_folder;
