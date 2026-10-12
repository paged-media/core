/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! The editor's save path leaves an untouched `Resources/Styles.xml` as it
//! came: open an InDesign-written package, edit a story's text, save as
//! `.paged` — the styles part goes back byte for byte.
//!
//! It did not: the style reader missed InDesign's spelling of a paragraph
//! style's tab stops (`<TabList type="list">` of `<ListItem
//! type="record">`), so the model's style had none and the export dropped
//! the source `<TabList>`. The style below is the one that lost it, as
//! InDesign 20.0.1 wrote it.

use std::io::{Cursor, Read, Write};

use paged_canvas::channel::Mutation;
use paged_canvas::{CanvasModel, CanvasOptions};

const PKG: &str = r#"xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging""#;

const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>
<idPkg:Styles xmlns:idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\" DOMVersion=\"20.0\">
\t<RootCharacterStyleGroup Self=\"u7a\">
\t\t<CharacterStyle Self=\"CharacterStyle/$ID/[No character style]\" Name=\"$ID/[No character style]\" />
\t</RootCharacterStyleGroup>
\t<RootParagraphStyleGroup Self=\"u79\">
\t\t<ParagraphStyle Self=\"ParagraphStyle/$ID/[No paragraph style]\" Name=\"$ID/[No paragraph style]\" />
\t\t<ParagraphStyle Self=\"ParagraphStyle/docx defaults\" Name=\"docx defaults\" Imported=\"false\" NextStyle=\"ParagraphStyle/docx defaults\" PointSize=\"11\" />
\t\t<ParagraphStyle Self=\"ParagraphStyle/docx direct format 9\" Name=\"docx direct format 9\" Imported=\"false\" NextStyle=\"ParagraphStyle/docx direct format 9\" SplitDocument=\"false\" EmitCss=\"true\" StyleUniqueId=\"a48c83b0-91fb-49bb-b749-03cb1ea60b74\" IncludeClass=\"true\" ExtendedKeyboardShortcut=\"0 0 0\" EmptyNestedStyles=\"true\" EmptyLineStyles=\"true\" EmptyGrepStyles=\"true\" KeyboardShortcut=\"0 0\">
\t\t\t<Properties>
\t\t\t\t<BasedOn type=\"object\">ParagraphStyle/docx defaults</BasedOn>
\t\t\t\t<PreviewColor type=\"enumeration\">Nothing</PreviewColor>
\t\t\t\t<TabList type=\"list\">
\t\t\t\t\t<ListItem type=\"record\">
\t\t\t\t\t\t<Alignment type=\"enumeration\">RightAlign</Alignment>
\t\t\t\t\t\t<AlignmentCharacter type=\"string\">.</AlignmentCharacter>
\t\t\t\t\t\t<Leader type=\"string\">.</Leader>
\t\t\t\t\t\t<Position type=\"unit\">216</Position>
\t\t\t\t\t</ListItem>
\t\t\t\t</TabList>
\t\t\t</Properties>
\t\t</ParagraphStyle>
\t</RootParagraphStyleGroup>
</idPkg:Styles>";

fn package() -> Vec<u8> {
    let designmap = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0" Self="d" {PKG}>
<idPkg:Styles src="Resources/Styles.xml"/>
<Layer Self="ub6" Name="Layer 1" Visible="true" Locked="false" Printable="true"/>
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
<idPkg:Story src="Stories/Story_st1.xml"/>
</Document>"#
    );
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread {PKG} DOMVersion="20.0">
<Spread Self="s1" PageCount="1" BindingLocation="0" ItemTransform="1 0 0 1 0 0">
<Page Self="p1" Name="1" GeometricBounds="0 0 720 540" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="st1" ItemTransform="1 0 0 1 0 0"><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray><PathPointType Anchor="48 48" LeftDirection="48 48" RightDirection="48 48"/><PathPointType Anchor="48 200" LeftDirection="48 200" RightDirection="48 200"/><PathPointType Anchor="480 200" LeftDirection="480 200" RightDirection="480 200"/><PathPointType Anchor="480 48" LeftDirection="480 48" RightDirection="480 48"/></PathPointArray></GeometryPathType></PathGeometry></Properties></TextFrame>
</Spread></idPkg:Spread>"#
    );
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story {PKG} DOMVersion="20.0">
<Story Self="st1"><ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/docx direct format 9"><CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]"><Content>The Picture</Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"#
    );
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        for (name, body) in [
            ("designmap.xml", designmap.as_str()),
            ("Resources/Styles.xml", STYLES),
            ("Spreads/Spread_s1.xml", spread.as_str()),
            ("Stories/Story_st1.xml", story.as_str()),
        ] {
            zip.start_file(name, deflated).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn entry(package: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).expect("zip");
    let mut e = zip.by_name(name).unwrap_or_else(|_| panic!("{name}"));
    let mut out = String::new();
    e.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn a_text_edit_saved_as_paged_leaves_the_styles_part_byte_identical() {
    let mut model =
        CanvasModel::load("styles-untouched", &package(), CanvasOptions::default()).expect("load");
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "st1".into(),
            offset: 0,
            text: "New ".into(),
            cell: None,
        })
        .expect("insert text");
    let saved = model
        .export_paged(paged_canvas::channel::PROTOCOL_VERSION.0)
        .expect("export .paged");

    assert!(
        entry(&saved, "Stories/Story_st1.xml").contains("New The Picture"),
        "the edit reached the story part"
    );
    let styles = entry(&saved, "Resources/Styles.xml");
    assert!(
        styles.contains("<Position type=\"unit\">216</Position>"),
        "the style's TabList was dropped:\n{styles}"
    );
    assert!(
        styles == STYLES,
        "Resources/Styles.xml was rewritten:\n{styles}"
    );
}

#[test]
fn the_style_s_tab_stop_reaches_the_model() {
    let model =
        CanvasModel::load("styles-tabs", &package(), CanvasOptions::default()).expect("load");
    let style = &model.scene().styles.paragraph_styles["ParagraphStyle/docx direct format 9"];
    assert_eq!(style.tab_list.len(), 1, "{:?}", style.tab_list);
    assert_eq!(style.tab_list[0].position, 216.0);
    assert_eq!(style.tab_list[0].leader.as_deref(), Some("."));
}
