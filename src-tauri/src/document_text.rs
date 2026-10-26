//! Bounded, read-only Office text snapshots shared by file reads and attachments.
//! Never executes Office, extracts to disk, follows links or recalculates formulas.

use std::path::Path;

mod docx;
mod package;
mod xlsx;

pub(crate) const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_TEXT_BYTES: usize = 16 * 1024;
pub(crate) const DOCX_NOTICE: &str = "【DOCX 正文文字快照】仅含正文段落与表格文字；不含图片、文本框、页眉页脚、批注、脚注/尾注、自动编号或版式。复杂域与修订不解析。\n\n";
pub(crate) const XLSX_NOTICE: &str = "【XLSX 单元格值快照】按工作表和坐标列出保存的原始值及合并范围，不是屏幕显示；数字未按日期、百分比或货币格式换算。不含图表、图片、批注、样式或拼音注释；不计算公式、不访问外部链接。含公式或隐藏工作表/行/列的文件不读取。\n\n";

#[derive(Clone, Copy)]
pub(crate) enum DocumentFormat {
    Docx,
    Xlsx,
}

impl DocumentFormat {
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "docx" => Some(Self::Docx),
            "xlsx" => Some(Self::Xlsx),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Docx => "DOCX",
            Self::Xlsx => "XLSX",
        }
    }

    pub(crate) fn extract(self, bytes: &[u8]) -> Result<String, String> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(format!("{} 原文件不能超过 4 MiB。", self.label()));
        }
        let (notice, body) = match self {
            Self::Docx => (
                DOCX_NOTICE,
                docx::extract(bytes, MAX_TEXT_BYTES - DOCX_NOTICE.len())?,
            ),
            Self::Xlsx => (
                XLSX_NOTICE,
                xlsx::extract(bytes, MAX_TEXT_BYTES - XLSX_NOTICE.len())?,
            ),
        };
        if body
            .bytes()
            .any(|b| b == 127 || (b < 32 && !matches!(b, b'\t' | b'\r' | b'\n')))
        {
            return Err("文档包含非文本控制字符，请另存为纯文本后重试。".into());
        }
        Ok(format!("{notice}{body}"))
    }
}
