//! Shared bounded Office ZIP access. Never extracts files or follows relations.
use std::collections::HashSet;
use std::io::{Cursor, Read};
use zip::ZipArchive;

pub(super) struct Package<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    format: &'static str,
}

#[cfg(test)]
pub(super) fn fixture(parts: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in parts {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

impl<'a> Package<'a> {
    pub(super) fn open(bytes: &'a [u8], format: &'static str) -> Result<Self, String> {
        let bad = || format!("{format} 文件损坏、已加密或不是受支持的标准文档，请另存后重试。");
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| bad())?;
        if archive.len() > 256 {
            return Err(format!("{format} 内部条目过多，请选择较小的文件。"));
        }
        let mut expanded = 0_u64;
        let mut names = HashSet::new();
        for index in 0..archive.len() {
            let file = archive.by_index_raw(index).map_err(|_| bad())?;
            expanded = expanded.saturating_add(file.size());
            if expanded > 16 * 1024 * 1024 {
                return Err(format!("{format} 解压后过大，请选择较小的文件。"));
            }
            if file.encrypted()
                || file.enclosed_name().is_none()
                || !names.insert(file.name().to_string())
                || file.name().to_ascii_lowercase().ends_with("vbaproject.bin")
            {
                return Err(bad());
            }
        }
        Ok(Self { archive, format })
    }

    pub(super) fn part(&mut self, name: &str, limit: usize) -> Result<String, String> {
        let bad = || format!("{} 内部数据损坏或缺失，请另存后重试。", self.format);
        let large = || format!("{} 内部文字数据过大，请选择较小的文件。", self.format);
        let file = self.archive.by_name(name).map_err(|_| bad())?;
        if file.size() > limit as u64 {
            return Err(large());
        }
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| bad())?;
        if bytes.len() > limit {
            return Err(large());
        }
        String::from_utf8(bytes).map_err(|_| {
            format!(
                "{} 内部文字不是 UTF-8，请另存为标准文档后重试。",
                self.format
            )
        })
    }
}
