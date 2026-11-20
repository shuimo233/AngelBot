use lopdf::{Document, LoadOptions};

const MAX_PAGES: usize = 40;
const MAX_STREAM_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn extract(bytes: &[u8], max_text_bytes: usize) -> Result<String, String> {
    let document = Document::load_mem_with_options(
        bytes,
        LoadOptions {
            strict: true,
            max_decompressed_size: Some(MAX_STREAM_BYTES),
            ..LoadOptions::default()
        },
    )
    .map_err(|_| "无法读取 PDF；请核对文件是否完整、未加密后重试。")?;
    if document.is_encrypted() {
        return Err("暂不支持加密 PDF。".into());
    }
    let pages = document.get_pages();
    if pages.len() > MAX_PAGES {
        return Err("PDF 最多读取 40 页，请先导出所需页面。".into());
    }
    let mut result = String::new();
    for page in pages.keys() {
        let text = document
            .extract_text_with_limit(&[*page], MAX_STREAM_BYTES)
            .map_err(|_| format!("无法提取 PDF 第 {page} 页的文字，请先核对原件。"))?;
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let heading = format!("【第 {page} 页】\n");
        if result.len() + heading.len() + text.len() + 2 > max_text_bytes {
            return Err("PDF 提取文字超过 16 KiB，请先导出所需页面。".into());
        }
        result.push_str(&heading);
        result.push_str(text);
        result.push_str("\n\n");
    }
    if result.is_empty() {
        return Err("PDF 中未找到可提取的文字。扫描件可先 OCR 或导出为文本。".into());
    }
    result.truncate(result.len() - 2);
    Ok(result)
}

#[cfg(test)]
pub(crate) fn fixture(page_texts: &[&str]) -> Vec<u8> {
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Object, Stream};

    let mut document = Document::with_version("1.5");
    let pages = document.new_object_id();
    let font = document.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let mut children = Vec::new();
    for text in page_texts {
        let operations = if text.is_empty() {
            Vec::new()
        } else {
            vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![30.into(), 750.into()]),
                Operation::new("Tj", vec![Object::string_literal(*text)]),
                Operation::new("ET", vec![]),
            ]
        };
        let contents = document.add_object(Stream::new(
            dictionary! {},
            Content { operations }.encode().unwrap(),
        ));
        let page = document.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => contents,
        });
        children.push(Object::Reference(page));
    }
    document.objects.insert(
        pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Count" => children.len() as i64, "Kids" => children,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        }),
    );
    let root = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    document.trailer.set("Root", root);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_snapshot_keeps_page_numbers_and_rejects_unreadable_or_oversized_inputs() {
        let bytes = fixture(&["Coupon due", "", "Final balance"]);
        assert_eq!(
            extract(&bytes, 1024).unwrap(),
            "【第 1 页】\nCoupon due\n\n【第 3 页】\nFinal balance"
        );
        assert!(extract(&bytes, 20).unwrap_err().contains("16 KiB"));
        assert!(extract(&fixture(&["Page"; MAX_PAGES + 1]), 1024)
            .unwrap_err()
            .contains("40 页"));
        assert!(extract(&fixture(&[""]), 1024).unwrap_err().contains("OCR"));
        assert!(extract(b"not a PDF", 1024).is_err());
    }

    #[test]
    fn pdf_snapshot_extracts_chinese_and_numbers_from_a_type0_font() {
        use lopdf::{dictionary, Stream};

        let mut document = Document::load_mem(&fixture(&["Placeholder"])).unwrap();
        let font = document
            .objects
            .iter()
            .find(|(_, object)| object.as_dict().is_ok_and(|dict| dict.has_type(b"Font")))
            .map(|(id, _)| *id)
            .unwrap();
        let cid_system = dictionary! {
            "Registry" => lopdf::Object::string_literal("Adobe"),
            "Ordering" => lopdf::Object::string_literal("Identity"),
            "Supplement" => 0,
        };
        let descendant = document.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType0", "BaseFont" => "STSong-Light",
            "CIDSystemInfo" => cid_system,
        });
        let cmap = document.add_object(Stream::new(
            dictionary! {},
            br#"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /AngelBotTestUnicode def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
9 beginbfchar
<0001> <5E94>
<0002> <4ED8>
<0003> <91D1>
<0004> <989D>
<0005> <0020>
<0006> <0032>
<0007> <0030>
<0008> <002E>
<0009> <0035>
endbfchar
endcmap
CMapName currentdict /CMap defineresource pop
end
end
"#
            .to_vec(),
        ));
        document.objects.insert(
            font,
            dictionary! {
                "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "STSong-Light",
                "Encoding" => "Identity-H", "DescendantFonts" => vec![descendant.into()],
                "ToUnicode" => cmap,
            }
            .into(),
        );
        let page = document.get_pages()[&1];
        let content = document.get_page_contents(page)[0];
        document
            .get_object_mut(content)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .set_content(
                b"BT /F1 12 Tf 30 750 Td <00010002000300040005000600070007000800090007> Tj ET"
                    .to_vec(),
            );
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        assert_eq!(
            extract(&bytes, 1024).unwrap(),
            "【第 1 页】\n应付金额 200.50"
        );
    }
}
