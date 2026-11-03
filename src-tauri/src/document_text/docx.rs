//! Bounded, in-memory WordprocessingML text extraction, not an Office renderer.
//! No archive entry is extracted to disk and no relationship is dereferenced.

use super::package::Package;
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};

const WORD_NS: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const STRICT_WORD_NS: &[u8] = b"http://purl.oclc.org/ooxml/wordprocessingml/main";
const BAD_DOCX: &str =
    "DOCX 文件损坏、已加密或不是受支持的标准文档，请另存为 DOCX 或纯文本后重试。";
const COMPLEX_DOCX: &str =
    "DOCX 含域、修订或外部导入等复杂内容，暂不能可靠读取；请导出为纯文本后重试。";
const XML_LIMIT: usize = 512 * 1024;

/// Confirm the standard main part rather than treating any renamed ZIP as DOCX.
fn declares_main(xml: &str, relationships: bool) -> Result<bool, String> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let expected_ns = if relationships {
        b"http://schemas.openxmlformats.org/package/2006/relationships".as_slice()
    } else {
        b"http://schemas.openxmlformats.org/package/2006/content-types".as_slice()
    };
    let mut found = false;
    let mut depth = 0_usize;
    let mut root_seen = false;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(|_| BAD_DOCX)?;
        let correct_ns = matches!(ns, ResolveResult::Bound(value) if value.as_ref() == expected_ns);
        match event {
            Event::Start(tag) => {
                depth += 1;
                if depth > 32 {
                    return Err(BAD_DOCX.into());
                }
                let root: &[u8] = if relationships {
                    b"Relationships"
                } else {
                    b"Types"
                };
                if depth == 1 {
                    if root_seen || !correct_ns || tag.local_name().as_ref() != root {
                        return Err(BAD_DOCX.into());
                    }
                    root_seen = true;
                }
                if depth != 2 || !correct_ns {
                    continue;
                }
                let mut values = std::collections::HashMap::new();
                for attribute in tag.attributes() {
                    let attribute = attribute.map_err(|_| BAD_DOCX)?;
                    values.insert(
                        attribute.key.as_ref().to_vec(),
                        attribute
                            .unescape_value()
                            .map_err(|_| BAD_DOCX)?
                            .into_owned(),
                    );
                }
                let value = |key: &[u8]| values.get(key).map(String::as_str);
                let matches_main = if relationships && tag.local_name().as_ref() == b"Relationship"
                    && matches!(value(b"Type"), Some("http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" | "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument")) {
                    if !matches!(value(b"Target"), Some("word/document.xml" | "/word/document.xml"))
                        || !value(b"TargetMode").is_none_or(|mode| mode == "Internal") { return Err(BAD_DOCX.into()); }
                    true
                } else if !relationships && tag.local_name().as_ref() == b"Override" {
                    if value(b"PartName") != Some("/word/document.xml") { continue; }
                    if value(b"ContentType") != Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml") { return Err(BAD_DOCX.into()); }
                    true
                } else { false };
                if matches_main {
                    if found {
                        return Err(BAD_DOCX.into());
                    }
                    found = true;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or(BAD_DOCX)?;
            }
            Event::DocType(_) => return Err(BAD_DOCX.into()),
            Event::Decl(decl) => validate_encoding(&decl)?,
            Event::Text(value)
                if depth == 0 && !value.xml_content().map_err(|_| BAD_DOCX)?.trim().is_empty() =>
            {
                return Err(BAD_DOCX.into())
            }
            Event::Eof => {
                return if depth == 0 {
                    Ok(found)
                } else {
                    Err(BAD_DOCX.into())
                }
            }
            _ => {}
        }
    }
}

fn validate_encoding(decl: &quick_xml::events::BytesDecl<'_>) -> Result<(), String> {
    if let Some(encoding) = decl.encoding() {
        if !encoding
            .map_err(|_| BAD_DOCX)?
            .eq_ignore_ascii_case(b"UTF-8")
        {
            return Err("DOCX 内部文字不是 UTF-8，请另存为标准 DOCX 后重试。".into());
        }
    }
    Ok(())
}

pub(super) fn extract(bytes: &[u8], text_limit: usize) -> Result<String, String> {
    let mut package = Package::open(bytes, "DOCX")?;
    if !declares_main(&package.part("[Content_Types].xml", 64 * 1024)?, false)?
        || !declares_main(&package.part("_rels/.rels", 64 * 1024)?, true)?
    {
        return Err(BAD_DOCX.into());
    }
    let xml = package.part("word/document.xml", XML_LIMIT)?;
    let mut reader = NsReader::from_str(&xml);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0_usize;
    let mut root_seen = false;
    let mut body = false;
    let mut body_seen = false;
    let mut text_depth = None;
    let mut skip_depth = None;
    let mut text = String::new();
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(|_| BAD_DOCX)?;
        let word = matches!(ns, ResolveResult::Bound(value) if value.as_ref() == WORD_NS || value.as_ref() == STRICT_WORD_NS);
        match event {
            Event::Start(tag) => {
                depth += 1;
                if depth > 64 || text_depth.is_some() {
                    return Err(BAD_DOCX.into());
                }
                let local = tag.local_name();
                let name = local.as_ref();
                if depth == 1 {
                    if root_seen || !word || name != b"document" {
                        return Err(BAD_DOCX.into());
                    }
                    root_seen = true;
                }
                if word && name == b"body" {
                    if depth != 2 || body_seen {
                        return Err(BAD_DOCX.into());
                    }
                    body = true;
                    body_seen = true;
                }
                if !body || skip_depth.is_some() {
                    continue;
                }
                if word && matches!(name, b"drawing" | b"pict" | b"object" | b"txbxContent") {
                    skip_depth = Some(depth);
                    continue;
                }
                if (word
                    && matches!(
                        name,
                        b"del"
                            | b"ins"
                            | b"moveFrom"
                            | b"moveTo"
                            | b"fldChar"
                            | b"fldSimple"
                            | b"instrText"
                            | b"delText"
                            | b"altChunk"
                            | b"subDoc"
                            | b"vanish"
                            | b"webHidden"
                            | b"sym"
                    ))
                    || name == b"AlternateContent"
                {
                    return Err(COMPLEX_DOCX.into());
                }
                if word {
                    match name {
                        b"t" => text_depth = Some(depth),
                        b"tab" | b"ptab" => text.push('\t'),
                        b"noBreakHyphen" => text.push('\u{2011}'),
                        b"softHyphen" => text.push('\u{ad}'),
                        b"br" | b"cr" => text.push('\n'),
                        _ => {}
                    }
                }
            }
            Event::End(tag) => {
                if skip_depth == Some(depth) {
                    skip_depth = None;
                } else if skip_depth.is_none() && word && body {
                    match tag.local_name().as_ref() {
                        b"t" => text_depth = None,
                        b"p" => text.push('\n'),
                        b"tc" => {
                            while text.ends_with('\n') {
                                text.pop();
                            }
                            text.push('\t');
                        }
                        b"tr" => {
                            while text.ends_with('\t') {
                                text.pop();
                            }
                            text.push('\n');
                        }
                        b"body" => body = false,
                        _ => {}
                    }
                }
                depth = depth.checked_sub(1).ok_or(BAD_DOCX)?;
            }
            Event::Text(value) if text_depth.is_some() => {
                text.push_str(&value.xml_content().map_err(|_| BAD_DOCX)?)
            }
            Event::CData(value) if text_depth.is_some() => {
                text.push_str(&value.xml_content().map_err(|_| BAD_DOCX)?)
            }
            Event::GeneralRef(value) if text_depth.is_some() => {
                if let Some(ch) = value.resolve_char_ref().map_err(|_| BAD_DOCX)? {
                    text.push(ch);
                } else {
                    let name = value.decode().map_err(|_| BAD_DOCX)?;
                    text.push_str(match name.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "apos" => "'",
                        "quot" => "\"",
                        _ => return Err(BAD_DOCX.into()),
                    });
                }
            }
            Event::DocType(_) => return Err(BAD_DOCX.into()),
            Event::Decl(decl) => validate_encoding(&decl)?,
            Event::Text(value)
                if depth == 0 && !value.xml_content().map_err(|_| BAD_DOCX)?.trim().is_empty() =>
            {
                return Err(BAD_DOCX.into())
            }
            Event::Eof => {
                if depth != 0 || !body_seen || text.trim().is_empty() {
                    return Err("DOCX 没有可读取的正文文字，或文档结构不完整。".into());
                }
                // The XML input already caps memory. Charge the final snapshot,
                // not paragraph/cell separators which are removed at EOF.
                let text = text.trim_end_matches(['\n', '\t']);
                if text.len() > text_limit {
                    return Err(
                        "DOCX 提取后的文字超过 16 KiB，不会截断；请先选择较小的内容。".into(),
                    );
                }
                return Ok(text.to_string());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::package::fixture as package;
    use super::*;
    use base64::Engine;

    const TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

    fn document(body: &str, ns: &[u8]) -> String {
        format!(
            "<w:document xmlns:w=\"{}\"><w:body>{body}</w:body></w:document>",
            std::str::from_utf8(ns).unwrap()
        )
    }

    fn docx(xml: &str) -> Vec<u8> {
        package(&[
            ("[Content_Types].xml", TYPES),
            ("_rels/.rels", RELS),
            ("word/document.xml", xml),
        ])
    }

    #[test]
    fn docx_snapshot_preserves_body_runs_entities_and_tables_but_never_reads_other_parts() {
        for ns in [WORD_NS, STRICT_WORD_NS] {
            let xml = document(
                r#"<w:p><w:r><w:t xml:space="preserve"> 今天 </w:t><w:t>&amp;&lt;&#x4E2D;&#25991;</w:t><w:tab/><w:t><![CDATA[A&B]]></w:t><w:br/><w:noBreakHyphen/><w:softHyphen/></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>事项</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>完成</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:drawing><w:txbxContent><w:p><w:r><w:t>secret-text-box</w:t></w:r></w:p></w:txbxContent></w:drawing><w:t>正文</w:t></w:r></w:p>"#,
                ns,
            );
            let bytes = package(&[
                ("[Content_Types].xml", TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", &xml),
                ("word/header1.xml", "secret-header"),
            ]);
            let snapshot =
                crate::commands::foreground_text_attachments::extract_document_text_impl(
                    serde_json::from_value(serde_json::json!({
                        "name": "今日安排.DOCX",
                        "dataBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    }))
                    .unwrap(),
                )
                .unwrap();
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("今日安排.DOCX"), &bytes).unwrap();
            assert_eq!(
                crate::commands::file::read_file_core(dir.path(), "今日安排.DOCX").unwrap(),
                snapshot.text
            );
            let edited = crate::agent::tool::ToolHandler::execute(
                &crate::agent::handlers::EditFileHandler,
                &serde_json::json!({"path":"今日安排.DOCX","old_string":"正文","new_string":"changed"}),
                dir.path(),
            );
            assert!(!edited.success);
            assert!(edited.content.contains("不能用文本写入"));
            assert_eq!(
                std::fs::read(dir.path().join("今日安排.DOCX")).unwrap(),
                bytes
            );
            assert_eq!(snapshot.name, "今日安排.DOCX");
            assert_eq!(
                snapshot.text,
                format!(
                    "{} 今天 &<中文\tA&B\n\u{2011}\u{ad}\n事项\t完成\n正文",
                    super::super::DOCX_NOTICE
                )
            );
            assert!(!snapshot.text.contains("secret"));
        }
        let default_ns = format!(
            "<document xmlns=\"{}\"><body><p><r><t>default namespace</t></r></p></body></document>",
            std::str::from_utf8(WORD_NS).unwrap()
        );
        assert_eq!(
            extract(&docx(&default_ns), 1024).unwrap(),
            "default namespace"
        );
        assert_eq!(
            extract(
                &docx(&format!(
                    "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>{default_ns}"
                )),
                17
            )
            .unwrap(),
            "default namespace"
        );
    }

    #[test]
    fn docx_rejects_unreliable_or_oversized_content_instead_of_returning_partial_text() {
        let valid = document("<w:p><w:r><w:t>hello</w:t></w:r></w:p>", WORD_NS);
        for body in [
            "",
            "<w:p/>",
            "<w:p><w:r><w:t>&unknown;</w:t></w:r></w:p>",
            "<w:ins><w:p><w:r><w:t>revision</w:t></w:r></w:p></w:ins>",
            "<w:p><w:fldSimple/></w:p>",
            "<w:p><w:r><w:fldChar/></w:r></w:p>",
            "<w:altChunk/>",
            "<w:p><w:r><w:vanish/><w:t>hidden</w:t></w:r></w:p>",
            "<w:p><w:r><w:sym/></w:r></w:p>",
            "<w:p><w:r><w:t>before</w:t><w:instrText>field</w:instrText></w:r></w:p>",
        ] {
            assert!(
                extract(&docx(&document(body, WORD_NS)), 1024).is_err(),
                "{body}"
            );
        }
        for xml in [
            format!("<!DOCTYPE document [<!ENTITY secret 'private'>]>{valid}"),
            format!("{valid}{valid}"),
            valid.replace("</w:p>", "</w:r>"),
            valid.replace("</w:body>", ""),
            valid.replace("w:body", "w:not-body"),
            valid.replace(std::str::from_utf8(WORD_NS).unwrap(), "urn:spoof"),
            format!("<?xml version=\"1.0\" encoding=\"UTF-16\"?>{valid}"),
        ] {
            assert!(extract(&docx(&xml), 1024).is_err());
        }
        assert!(extract(b"not a ZIP or encrypted Office document", 1024).is_err());
        assert!(extract(&package(&[("word/document.xml", &valid)]), 1024).is_err());
        for (types, rels) in [
            (
                TYPES.replace("document.main+xml", "template.main+xml"),
                RELS.to_string(),
            ),
            (
                TYPES.to_string(),
                RELS.replace("Target=", "TargetMode=\"External\" Target="),
            ),
            (
                TYPES.to_string(),
                RELS.replace("word/document.xml", "https://private.invalid/document.xml"),
            ),
            (TYPES.to_string(), format!("{RELS}{RELS}")),
        ] {
            assert!(extract(
                &package(&[
                    ("[Content_Types].xml", &types),
                    ("_rels/.rels", &rels),
                    ("word/document.xml", &valid)
                ]),
                1024
            )
            .is_err());
        }
        for extra in ["../private.txt", "word/vbaProject.bin"] {
            assert!(extract(
                &package(&[
                    ("[Content_Types].xml", TYPES),
                    ("_rels/.rels", RELS),
                    ("word/document.xml", &valid),
                    (extra, "ignored")
                ]),
                1024
            )
            .is_err());
        }
        assert!(extract(&docx(&valid), 4).unwrap_err().contains("不会截断"));
        assert_eq!(extract(&docx(&valid), 5).unwrap(), "hello");
        assert!(extract(&docx(&format!("{valid}{}", " ".repeat(XML_LIMIT))), 1024).is_err());
        let parts: Vec<_> = (0..257).map(|i| (format!("part{i}"), "x")).collect();
        assert!(extract(
            &package(
                &parts
                    .iter()
                    .map(|(name, text)| (name.as_str(), *text))
                    .collect::<Vec<_>>()
            ),
            1024
        )
        .is_err());
        let expanded = "x".repeat(16 * 1024 * 1024 + 1);
        assert!(extract(&package(&[("image", &expanded)]), 1024).is_err());
    }
}
