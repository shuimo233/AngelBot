//! Stored cell values, not an Excel renderer or calculation engine.
//! Sparse coordinates stay sparse; no grid is allocated from worksheet dimensions.
use super::package::Package;
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use std::collections::{HashMap, HashSet};

const SHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const STRICT_NS: &str = "http://purl.oclc.org/ooxml/spreadsheetml/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const OFFICE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
const STRICT_REL: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships/";
const BAD: &str = "XLSX 文件结构损坏或不受支持，请另存为 XLSX 或 CSV 后重试。";
const COMPLEX: &str =
    "XLSX 含公式、隐藏内容、外部关系或复杂扩展，暂不能可靠读取；请导出所需工作表为 CSV 后重试。";
const LIMIT: &str = "XLSX 内容超过读取预算，不会截断；请先选择较小的内容或导出所需工作表为 CSV。";
const XML_LIMIT: usize = 512 * 1024;

#[derive(Default)]
struct Node {
    name: String,
    attrs: HashMap<String, String>,
    relationship_id: Option<String>,
    text: String,
    children: Vec<Node>,
}
impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }
    fn child(&self, name: &str) -> Result<Option<&Self>, String> {
        let mut found = self.children.iter().filter(|node| node.name == name);
        let first = found.next();
        if found.next().is_some() {
            return Err(BAD.into());
        }
        Ok(first)
    }
}

// Bounded tree makes the structural distinctions (row/cell/value/rich text)
// explicit. Namespaces and the whole XML are validated before values are read.
fn xml(xml: &str, root: &str, namespaces: &[&str]) -> Result<Node, String> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Node> = Vec::new();
    let mut result = None;
    let mut count = 0;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(|_| BAD)?;
        let valid_ns = matches!(ns, ResolveResult::Bound(value) if namespaces.iter().any(|ns| value.as_ref() == ns.as_bytes()));
        match event {
            Event::Start(tag) => {
                count += 1;
                if count > 16384 || stack.len() >= 32 {
                    return Err(LIMIT.into());
                }
                if !valid_ns {
                    return Err(COMPLEX.into());
                }
                let name = std::str::from_utf8(tag.local_name().as_ref())
                    .map_err(|_| BAD)?
                    .to_string();
                if stack.is_empty() && (result.is_some() || name != root) {
                    return Err(BAD.into());
                }
                let mut node = Node {
                    name,
                    ..Node::default()
                };
                for attr in tag.attributes() {
                    let attr = attr.map_err(|_| BAD)?;
                    let (ns, local) = reader.resolve_attribute(attr.key);
                    let value = attr.unescape_value().map_err(|_| BAD)?.into_owned();
                    match ns {
                        ResolveResult::Unbound => {
                            node.attrs.insert(
                                std::str::from_utf8(local.as_ref()).map_err(|_| BAD)?.into(),
                                value,
                            );
                        }
                        ResolveResult::Bound(ns)
                            if (ns.as_ref() == OFFICE_REL.trim_end_matches('/').as_bytes()
                                || ns.as_ref() == STRICT_REL.trim_end_matches('/').as_bytes())
                                && local.as_ref() == b"id" =>
                        {
                            if node.relationship_id.replace(value).is_some() {
                                return Err(BAD.into());
                            }
                        }
                        ResolveResult::Unknown(_) => return Err(BAD.into()),
                        _ => {}
                    }
                }
                stack.push(node);
            }
            Event::End(_) => {
                let node = stack.pop().ok_or(BAD)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    result = Some(node);
                }
            }
            Event::Text(value) => {
                let value = value.xml_content().map_err(|_| BAD)?;
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&value);
                } else if !value.trim().is_empty() {
                    return Err(BAD.into());
                }
            }
            Event::CData(value) => stack
                .last_mut()
                .ok_or(BAD)?
                .text
                .push_str(&value.xml_content().map_err(|_| BAD)?),
            Event::GeneralRef(value) => {
                let text = &mut stack.last_mut().ok_or(BAD)?.text;
                if let Some(ch) = value.resolve_char_ref().map_err(|_| BAD)? {
                    text.push(ch);
                } else {
                    text.push_str(match value.decode().map_err(|_| BAD)?.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "apos" => "'",
                        "quot" => "\"",
                        _ => return Err(BAD.into()),
                    });
                }
            }
            Event::DocType(_) => return Err(BAD.into()),
            Event::Decl(decl) => {
                if let Some(encoding) = decl.encoding() {
                    if !encoding.map_err(|_| BAD)?.eq_ignore_ascii_case(b"UTF-8") {
                        return Err(BAD.into());
                    }
                }
            }
            Event::Eof => {
                return if stack.is_empty() {
                    result.ok_or_else(|| BAD.into())
                } else {
                    Err(BAD.into())
                }
            }
            _ => {}
        }
    }
}

fn relation_type(value: Option<&str>, suffix: &str) -> bool {
    value.is_some_and(|value| {
        [OFFICE_REL, STRICT_REL]
            .iter()
            .any(|prefix| value == format!("{prefix}{suffix}"))
    })
}

fn part_path(target: &str, base: &str) -> Result<String, String> {
    if target.is_empty()
        || target.contains(['\\', ':', '%', '?', '#'])
        || target.chars().any(char::is_control)
        || target
            .trim_start_matches('/')
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(BAD.into());
    }
    Ok(if target.starts_with('/') {
        target[1..].into()
    } else {
        format!("{base}{target}")
    })
}

fn rich_text(node: &Node) -> Result<String, String> {
    fn collect(node: &Node, value: &mut String) -> Result<(), String> {
        for child in &node.children {
            match child.name.as_str() {
                "t" if child.children.is_empty() => value.push_str(&child.text),
                "r" => collect(child, value)?,
                "rPr" | "phoneticPr" | "rPh" => {}
                _ => return Err(COMPLEX.into()),
            }
        }
        Ok(())
    }
    let mut value = String::new();
    collect(node, &mut value)?;
    // Excel's escaped UTF-16 code units are not XML entities. Decode once, so
    // _x005F_x0041_ remains the literal _x0041_, not a second escape pass.
    let mut units = Vec::new();
    let mut rest = value.as_str();
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        if bytes.len() >= 7
            && bytes[0] == b'_'
            && bytes[1] == b'x'
            && bytes[6] == b'_'
            && bytes[2..6].iter().all(u8::is_ascii_hexdigit)
        {
            units.push(u16::from_str_radix(&rest[2..6], 16).map_err(|_| BAD)?);
            rest = &rest[7..];
        } else {
            let ch = rest.chars().next().ok_or(BAD)?;
            let mut encoded = [0; 2];
            units.extend_from_slice(ch.encode_utf16(&mut encoded));
            rest = &rest[ch.len_utf8()..];
        }
    }
    String::from_utf16(&units).map_err(|_| BAD.into())
}

fn coordinate(value: &str) -> Result<(u32, u32), String> {
    let digits = value.find(|ch: char| ch.is_ascii_digit()).ok_or(BAD)?;
    let (letters, row) = value.split_at(digits);
    if letters.is_empty()
        || letters.len() > 3
        || !letters.bytes().all(|ch| ch.is_ascii_uppercase())
        || row.starts_with('0')
        || !row.bytes().all(|ch| ch.is_ascii_digit())
    {
        return Err(BAD.into());
    }
    let column = letters
        .bytes()
        .fold(0_u32, |value, ch| value * 26 + u32::from(ch - b'A' + 1));
    let row: u32 = row.parse().map_err(|_| BAD)?;
    if !(1..=16384).contains(&column) || !(1..=1048576).contains(&row) {
        return Err(BAD.into());
    }
    Ok((row, column))
}

fn address(row: u32, mut column: u32) -> String {
    let mut letters = Vec::new();
    while column > 0 {
        column -= 1;
        letters.push(b'A' + (column % 26) as u8);
        column /= 26;
    }
    letters.reverse();
    format!(
        "{}{row}",
        String::from_utf8(letters).expect("ASCII coordinate")
    )
}

fn cell_value(cell: &Node, strings: &[String]) -> Result<Option<String>, String> {
    if cell
        .children
        .iter()
        .any(|child| !matches!(child.name.as_str(), "f" | "v" | "is"))
    {
        return Err(COMPLEX.into());
    }
    if cell.child("f")?.is_some() {
        return Err(COMPLEX.into());
    }
    let scalar = cell.child("v")?;
    let inline = cell.child("is")?;
    if scalar.is_some_and(|node| !node.children.is_empty()) {
        return Err(BAD.into());
    }
    if scalar.is_none() && inline.is_none() {
        return Ok(None);
    }
    let value = scalar.map(|node| node.text.as_str()).unwrap_or("");
    let value: String = match cell.attr("t").unwrap_or("n") {
        "inlineStr" if scalar.is_none() => rich_text(inline.ok_or(BAD)?)?,
        "s" if inline.is_none() => strings
            .get(value.parse::<usize>().map_err(|_| BAD)?)
            .ok_or(BAD)?
            .clone(),
        "b" if inline.is_none() => match value {
            "0" => "false".into(),
            "1" => "true".into(),
            _ => return Err(BAD.into()),
        },
        "n" if inline.is_none() && value.parse::<f64>().is_ok_and(f64::is_finite) => value.into(),
        "str" | "d" | "e" if inline.is_none() => value.into(),
        _ => return Err(COMPLEX.into()),
    };
    if value
        .bytes()
        .any(|byte| byte == 127 || (byte < 32 && !matches!(byte, b'\t' | b'\r' | b'\n')))
    {
        return Err(COMPLEX.into());
    }
    Ok(Some(value))
}

fn hidden(node: &Node) -> Result<bool, String> {
    match node.attr("hidden") {
        None | Some("0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        _ => Err(BAD.into()),
    }
}

pub(super) fn extract(bytes: &[u8], text_limit: usize) -> Result<String, String> {
    let mut package = Package::open(bytes, "XLSX")?;
    let types = xml(
        &package.part("[Content_Types].xml", 64 * 1024)?,
        "Types",
        &[TYPES_NS],
    )?;
    let main: Vec<_> = types
        .children
        .iter()
        .filter(|node| node.name == "Override" && node.attr("PartName") == Some("/xl/workbook.xml"))
        .collect();
    if main.len() != 1
        || main[0].attr("ContentType")
            != Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml")
    {
        return Err(BAD.into());
    }
    let root_rels = xml(
        &package.part("_rels/.rels", 64 * 1024)?,
        "Relationships",
        &[REL_NS],
    )?;
    let main: Vec<_> = root_rels
        .children
        .iter()
        .filter(|node| relation_type(node.attr("Type"), "officeDocument"))
        .collect();
    if main.len() != 1
        || !matches!(main[0].attr("TargetMode"), None | Some("Internal"))
        || part_path(main[0].attr("Target").ok_or(BAD)?, "")? != "xl/workbook.xml"
    {
        return Err(BAD.into());
    }
    let book = xml(
        &package.part("xl/workbook.xml", XML_LIMIT)?,
        "workbook",
        &[SHEET_NS, STRICT_NS],
    )?;
    let rels = xml(
        &package.part("xl/_rels/workbook.xml.rels", 64 * 1024)?,
        "Relationships",
        &[REL_NS],
    )?;
    let mut relations = HashMap::new();
    let mut shared = None;
    for relation in &rels.children {
        if relation.name != "Relationship"
            || !matches!(relation.attr("TargetMode"), None | Some("Internal"))
        {
            return Err(COMPLEX.into());
        }
        let id = relation.attr("Id").ok_or(BAD)?;
        let target = part_path(relation.attr("Target").ok_or(BAD)?, "xl/")?;
        if relations.insert(id, (relation, target.clone())).is_some() {
            return Err(BAD.into());
        }
        if relation_type(relation.attr("Type"), "sharedStrings") && shared.replace(target).is_some()
        {
            return Err(BAD.into());
        }
    }
    let mut strings = Vec::new();
    if let Some(path) = shared {
        let table = xml(
            &package.part(&path, XML_LIMIT)?,
            "sst",
            &[SHEET_NS, STRICT_NS],
        )?;
        for item in &table.children {
            if item.name != "si" {
                return Err(COMPLEX.into());
            }
            if strings.len() >= 8192 {
                return Err(LIMIT.into());
            }
            strings.push(rich_text(item)?);
        }
    }
    let sheets = book.child("sheets")?.ok_or(BAD)?;
    if sheets.children.is_empty() || sheets.children.len() > 4 {
        return Err(LIMIT.into());
    }
    let mut names = HashSet::new();
    let mut parts = HashSet::new();
    let mut output = String::new();
    let mut cell_count = 0;
    let mut has_values = false;
    for sheet in &sheets.children {
        if sheet.name != "sheet" || !matches!(sheet.attr("state"), None | Some("visible")) {
            return Err(COMPLEX.into());
        }
        let name = sheet
            .attr("name")
            .filter(|name| !name.trim().is_empty() && !name.chars().any(char::is_control))
            .ok_or(BAD)?;
        if !names.insert(name.to_lowercase()) {
            return Err(BAD.into());
        }
        let (relation, path) = relations
            .get(sheet.relationship_id.as_deref().ok_or(BAD)?)
            .ok_or(BAD)?;
        if !relation_type(relation.attr("Type"), "worksheet") || !parts.insert(path) {
            return Err(COMPLEX.into());
        }
        let data = xml(
            &package.part(path, XML_LIMIT)?,
            "worksheet",
            &[SHEET_NS, STRICT_NS],
        )?;
        if let Some(cols) = data.child("cols")? {
            for col in &cols.children {
                if col.name != "col" || hidden(col)? {
                    return Err(COMPLEX.into());
                }
            }
        }
        output.push_str(&format!(
            "工作表 {}\n",
            serde_json::to_string(name).map_err(|_| BAD)?
        ));
        let mut previous_row = 0;
        for row in &data.child("sheetData")?.ok_or(BAD)?.children {
            if row.name != "row" || hidden(row)? {
                return Err(COMPLEX.into());
            }
            let row_index = match row.attr("r") {
                Some(row) => row.parse::<u32>().map_err(|_| BAD)?,
                None => previous_row + 1,
            };
            if row_index <= previous_row || row_index > 1048576 {
                return Err(BAD.into());
            }
            previous_row = row_index;
            let mut previous_column = 0;
            for cell in &row.children {
                cell_count += 1;
                if cell_count > 4096 {
                    return Err(LIMIT.into());
                }
                if cell.name != "c" {
                    return Err(COMPLEX.into());
                }
                let inferred = address(row_index, previous_column + 1);
                let reference = cell.attr("r").unwrap_or(&inferred);
                let (cell_row, column) = coordinate(reference)?;
                if cell_row != row_index || column <= previous_column {
                    return Err(BAD.into());
                }
                previous_column = column;
                if let Some(value) = cell_value(cell, &strings)? {
                    has_values = true;
                    output.push_str(&format!(
                        "{reference} = {}\n",
                        serde_json::to_string(&value).map_err(|_| BAD)?
                    ));
                }
                if output.trim_end_matches('\n').len() > text_limit {
                    return Err(LIMIT.into());
                }
            }
        }
        if let Some(merges) = data.child("mergeCells")? {
            if merges.children.len() > 128 {
                return Err(LIMIT.into());
            }
            for merge in &merges.children {
                let reference = merge.attr("ref").ok_or(BAD)?;
                let (start, end) = reference.split_once(':').ok_or(BAD)?;
                let (a, b) = (coordinate(start)?, coordinate(end)?);
                if merge.name != "mergeCell" || a.0 > b.0 || a.1 > b.1 {
                    return Err(BAD.into());
                }
                output.push_str(&format!("合并范围 {reference}\n"));
            }
        }
        if output.trim_end_matches('\n').len() > text_limit {
            return Err(LIMIT.into());
        }
    }
    if !has_values {
        return Err("XLSX 没有可读取的单元格值，请选择含数据的工作簿。".into());
    }
    Ok(output.trim_end_matches('\n').into())
}

#[cfg(test)]
mod tests {
    use super::super::{package::fixture, XLSX_NOTICE};
    use super::*;
    use crate::commands::foreground_text_attachments::extract_document_text_impl;
    use base64::Engine;

    const TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
    const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    const BOOK: &str = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:rel="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="今日事项" sheetId="1" rel:id="sheet1"/></sheets></workbook>"#;
    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="sheet1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#;
    const STRINGS: &str = r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><r><t>今天 &amp; </t></r><r><t>完成_x005F_x0041_</t></r><rPh><t>IGNORED_PHONETIC</t></rPh></si><si><t>_xD83D__xDE00_</t></si></sst>"#;

    fn sheet(body: &str) -> String {
        format!("<worksheet xmlns=\"{SHEET_NS}\">{body}</worksheet>")
    }
    fn xlsx(body: &str, changed: Option<(&str, &str)>, extra: &[(&str, &str)]) -> Vec<u8> {
        let worksheet = sheet(body);
        let mut parts = vec![
            ("[Content_Types].xml", TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("xl/workbook.xml", BOOK),
            ("xl/_rels/workbook.xml.rels", RELS),
            ("xl/worksheets/sheet1.xml", worksheet.as_str()),
            ("xl/sharedStrings.xml", STRINGS),
        ];
        if let Some((path, text)) = changed {
            parts.iter_mut().find(|(name, _)| *name == path).unwrap().1 = text;
        }
        parts.extend_from_slice(extra);
        fixture(&parts)
    }

    #[test]
    fn stored_cells_preserve_sparse_coordinates_rich_strings_scalars_and_merges() {
        let body = r#"<dimension ref="A1:XFD1048576"/><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="inlineStr"><is><t><![CDATA[事项\n= A1]]></t></is></c><c r="D1"><v>123456789012345.67</v></c><c r="E1" t="b"><v>0</v></c><c r="F1" t="e"><v>#N/A</v></c></row><row r="1048576"><c r="XFD1048576" t="s"><v>1</v></c></row></sheetData><mergeCells><mergeCell ref="A1:B1"/></mergeCells>"#;
        let bytes = xlsx(
            body,
            None,
            &[
                ("xl/comments1.xml", "IGNORED_COMMENT"),
                ("xl/styles.xml", "IGNORED_STYLE"),
            ],
        );
        let expected = "工作表 \"今日事项\"\nA1 = \"今天 & 完成_x0041_\"\nC1 = \"事项\\\\n= A1\"\nD1 = \"123456789012345.67\"\nE1 = \"false\"\nF1 = \"#N/A\"\nXFD1048576 = \"😀\"\n合并范围 A1:B1";
        let snapshot = extract_document_text_impl(
            serde_json::from_value(serde_json::json!({
                "name": "今日安排.XLSX",
                "dataBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
            }))
            .unwrap(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("今日安排.XLSX"), &bytes).unwrap();
        assert_eq!(
            crate::commands::file::read_file_core(dir.path(), "今日安排.XLSX").unwrap(),
            snapshot.text
        );
        assert_eq!(snapshot.name, "今日安排.XLSX");
        assert_eq!(snapshot.text, format!("{XLSX_NOTICE}{expected}"));
        assert!(!snapshot.text.contains("IGNORED"));
        assert_eq!(extract(&bytes, expected.len()).unwrap(), expected);
        assert!(extract(&bytes, expected.len() - 1)
            .unwrap_err()
            .contains("不会截断"));
        let strict_book = BOOK.replace(SHEET_NS, STRICT_NS).replace(
            OFFICE_REL.trim_end_matches('/'),
            STRICT_REL.trim_end_matches('/'),
        );
        assert!(extract(
            &xlsx(body, Some(("xl/workbook.xml", &strict_book)), &[]),
            4096
        )
        .is_ok());
        let inferred = xlsx("<sheetData><row><c t=\"inlineStr\"><is><t>A</t></is></c><c><v>2</v></c></row></sheetData>", None, &[]);
        assert_eq!(
            extract(&inferred, 1024).unwrap(),
            "工作表 \"今日事项\"\nA1 = \"A\"\nB1 = \"2\""
        );
        let no_shared = xlsx("<sheetData><row r=\"1\"><c r=\"A1\"><v>0</v></c></row></sheetData>", Some(("xl/_rels/workbook.xml.rels", &RELS.replace("<Relationship Id=\"strings\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/>", ""))), &[]);
        assert!(extract(&no_shared, 1024).unwrap().ends_with("A1 = \"0\""));
        let book = BOOK.replace(
            "</sheets>",
            "<sheet name=\"明日事项\" sheetId=\"2\" rel:id=\"sheet2\"/></sheets>",
        );
        let rels = RELS.replace("</Relationships>", "<Relationship Id=\"sheet2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet2.xml\"/></Relationships>");
        let first = sheet("<sheetData><row><c><v>1</v></c></row></sheetData>");
        let second = sheet("<sheetData><row><c><v>2</v></c></row></sheetData>");
        let multi = fixture(&[
            ("[Content_Types].xml", TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("xl/workbook.xml", &book),
            ("xl/_rels/workbook.xml.rels", &rels),
            ("xl/sharedStrings.xml", STRINGS),
            ("xl/worksheets/sheet1.xml", &first),
            ("xl/worksheets/sheet2.xml", &second),
        ]);
        assert_eq!(
            extract(&multi, 1024).unwrap(),
            "工作表 \"今日事项\"\nA1 = \"1\"\n工作表 \"明日事项\"\nA1 = \"2\""
        );
        let available = super::super::MAX_TEXT_BYTES - XLSX_NOTICE.len();
        let overhead = "工作表 \"今日事项\"\nA1 = \"\"".len();
        for (extra, valid) in [(0, true), (1, false)] {
            let value = "a".repeat(available - overhead + extra);
            let body = format!(
                "<sheetData><row><c t=\"inlineStr\"><is><t>{value}</t></is></c></row></sheetData>"
            );
            let result = extract_document_text_impl(
                serde_json::from_value(serde_json::json!({
                    "name": "boundary.xlsx",
                    "dataBase64": base64::engine::general_purpose::STANDARD.encode(xlsx(
                        &body,
                        None,
                        &[],
                    )),
                }))
                .unwrap(),
            );
            if valid {
                assert_eq!(result.unwrap().text.len(), super::super::MAX_TEXT_BYTES);
            } else {
                assert!(result.unwrap_err().contains("不会截断"));
            }
        }
    }

    #[test]
    fn unsafe_unsupported_corrupt_and_over_budget_workbooks_never_return_partial_values() {
        let valid = "<sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData>";
        for body in [
            "<sheetData/>",
            "<sheetData><row r=\"1\"><c r=\"A1\"><f>1+1</f><v>2</v></c></row></sheetData>",
            "<sheetData><row hidden=\"1\" r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData>",
            "<cols><col min=\"1\" max=\"1\" hidden=\"true\"/></cols><sheetData/>",
            "<sheetData><row r=\"1\"><c r=\"A1\" t=\"s\"><v>999</v></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A0\"><v>1</v></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A2\"><v>1</v></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A1\"><v>NaN</v></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A1\"><v>1</v><v>2</v></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>_x0000_</t></is></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>_xD800_</t></is></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>&unknown;</t></is></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c><c r=\"A1\"><v>2</v></c></row></sheetData>",
            "<sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData><x:extension xmlns:x=\"urn:unknown\"/>",
        ] { assert!(extract(&xlsx(body, None, &[]), 4096).is_err(), "{body}"); }
        for (path, value) in [
            (
                "xl/workbook.xml",
                BOOK.replace("sheetId=", "state=\"hidden\" sheetId="),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                RELS.replace("worksheets/sheet1.xml", "../private.xml"),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                RELS.replace("Target=", "TargetMode=\"External\" Target="),
            ),
            (
                "xl/sharedStrings.xml",
                STRINGS.replace(SHEET_NS, "urn:spoof"),
            ),
            (
                "[Content_Types].xml",
                TYPES.replace("sheet.main+xml", "macroEnabled.main+xml"),
            ),
            (
                "_rels/.rels",
                ROOT_RELS.replace("xl/workbook.xml", "https://private.invalid"),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!("<!DOCTYPE worksheet>{}", sheet(valid)),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!("{}{}", sheet(valid), sheet(valid)),
            ),
            (
                "xl/worksheets/sheet1.xml",
                sheet(valid).replace("</row>", "</c>"),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!("{}{}", sheet(valid), " ".repeat(XML_LIMIT)),
            ),
        ] {
            assert!(
                extract(&xlsx(valid, Some((path, &value)), &[]), 4096).is_err(),
                "{path}"
            );
        }
        assert!(extract(b"not a workbook", 4096).is_err());
        for extra in ["../private.txt", "xl/vbaProject.bin"] {
            assert!(extract(&xlsx(valid, None, &[(extra, "x")]), 4096).is_err());
        }
        let too_many = format!(
            "<sheetData><row r=\"1\">{}</row></sheetData>",
            "<c/>".repeat(4097)
        );
        assert!(extract(&xlsx(&too_many, None, &[]), 65536)
            .unwrap_err()
            .contains("预算"));
        let mut too_many_sheets = String::new();
        for n in 1..=5 {
            too_many_sheets.push_str(&format!("<sheet name=\"{n}\" rel:id=\"sheet1\"/>"));
        }
        assert!(extract(
            &xlsx(
                valid,
                Some((
                    "xl/workbook.xml",
                    &BOOK.replace(
                        "<sheet name=\"今日事项\" sheetId=\"1\" rel:id=\"sheet1\"/>",
                        &too_many_sheets
                    )
                )),
                &[]
            ),
            4096
        )
        .is_err());
    }
}
