import type { TextAttachment } from '$types';
import { extractDocumentText } from './commands/message';

// Mirrored by the backend: these are UTF-8 byte limits, not character limits.
export const MAX_TEXT_ATTACHMENTS = 4;
export const MAX_TEXT_ATTACHMENT_BYTES = 16 * 1024;
export const MAX_TEXT_ATTACHMENTS_BYTES = 32 * 1024;
export const MAX_DOCUMENT_FILE_BYTES = 4 * 1024 * 1024;
const MAX_TEXT_SOURCE_BYTES = MAX_TEXT_ATTACHMENT_BYTES * 2 + 4;
export const TEXT_ATTACHMENT_EXTENSIONS = ['txt', 'md', 'csv', 'tsv', 'json', 'log', 'yaml', 'yml', 'docx', 'xlsx'];
export const TEXT_ATTACHMENT_ACCEPT = TEXT_ATTACHMENT_EXTENSIONS.map((extension) => `.${extension}`).join(',');

export function textAttachmentBytes(text: string): number {
  return new TextEncoder().encode(text).byteLength;
}

function validateName(name: string): void {
  if (!name.trim() || textAttachmentBytes(name) > 200 || /[\/\\\x00-\x1f\x7f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/u.test(name)) {
    throw new Error('文件名不符合要求，请使用不超过 200 字节、不含路径或控制字符的名称。');
  }
  const dot = name.lastIndexOf('.');
  const extension = name.slice(dot + 1).toLowerCase();
  if (dot <= 0 || !name.slice(0, dot).trim() || !TEXT_ATTACHMENT_EXTENSIONS.includes(extension)) {
    throw new Error('支持 UTF-8、带 BOM 的 UTF-16 文本、DOCX 正文和 XLSX 单元格值；图片、PDF、旧版 Word/Excel 暂不支持。');
  }
}

export function validateTextAttachments(files: TextAttachment[]): void {
  if (files.length > MAX_TEXT_ATTACHMENTS) throw new Error('一次最多添加 4 个文本文件。');
  let total = 0;
  for (const file of files) {
    validateName(file.name);
    const size = textAttachmentBytes(file.text);
    if (size > MAX_TEXT_ATTACHMENT_BYTES) throw new Error('每个文本文件最多 16 KiB，不会截断内容。');
    if (!file.text.trim()) throw new Error('不能添加空白文件，请选择包含内容的文本文件。');
    if (/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/u.test(file.text)) {
      throw new Error('文件包含二进制或控制字符，请另存为 UTF-8 文本后重试。');
    }
    total += size;
  }
  if (total > MAX_TEXT_ATTACHMENTS_BYTES) throw new Error('附件合计最多 32 KiB，不会截断内容。');
}

function readFile(file: File): Promise<ArrayBuffer> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => reader.result instanceof ArrayBuffer
      ? resolve(reader.result)
      : reject(new Error('无法读取文件，请重新选择。'));
    reader.onerror = () => reject(new Error('无法读取文件，请重新选择。'));
    reader.onabort = () => reject(new Error('文件读取已取消。'));
    reader.readAsArrayBuffer(file);
  });
}

function isDocument(name: string): boolean {
  return /\.(docx|xlsx)$/i.test(name);
}

function validateFileSize(name: string, size: number): void {
  if (isDocument(name)) {
    if (size > MAX_DOCUMENT_FILE_BYTES) throw new Error('DOCX/XLSX 原文件不能超过 4 MiB。');
  } else if (size > MAX_TEXT_SOURCE_BYTES) {
    throw new Error('文本原文件过大；解码后的文字每个最多 16 KiB，不会截断内容。');
  }
}

function base64(bytes: ArrayBuffer): string {
  const data = new Uint8Array(bytes);
  const chunks: string[] = [];
  for (let start = 0; start < data.length; start += 8192) {
    chunks.push(String.fromCharCode(...data.subarray(start, start + 8192)));
  }
  return btoa(chunks.join(''));
}

// Only use after name validation: never echo an invalid name containing a path
// or control characters. Office IPC already normalizes its fixed safe errors.
function fileReadError(name: string, error: unknown): Error {
  const reason = error instanceof Error ? error.message : '无法读取文件，请重新选择。';
  return new Error(`无法添加“${name}”：${reason}`);
}

/** Atomic bounded batch: no paths, lossy decoding, partial selection or truncation. */
export async function loadTextAttachments(files: File[], existing: TextAttachment[] = []): Promise<TextAttachment[]> {
  validateTextAttachments(existing);
  if (existing.length + files.length > MAX_TEXT_ATTACHMENTS) throw new Error('一次最多添加 4 个文本文件。');
  for (const file of files) {
    validateName(file.name);
    try { validateFileSize(file.name, file.size); }
    catch (error) { throw fileReadError(file.name, error); }
  }
  const loaded: TextAttachment[] = [];
  for (const file of files) {
    try {
      const bytes = await readFile(file);
      validateFileSize(file.name, bytes.byteLength);
      if (isDocument(file.name)) {
        const snapshot = await extractDocumentText(file.name, base64(bytes));
        if (snapshot.name !== file.name) throw new Error('文档读取结果与所选文件不符，请重新选择。');
        loaded.push(snapshot);
      } else {
        let text: string;
        const prefix = new Uint8Array(bytes, 0, Math.min(bytes.byteLength, 2));
        const encoding = prefix[0] === 0xff && prefix[1] === 0xfe ? 'utf-16le'
          : prefix[0] === 0xfe && prefix[1] === 0xff ? 'utf-16be' : 'utf-8';
        try { text = new TextDecoder(encoding, { fatal: true }).decode(bytes).replace(/^\uFEFF/u, ''); }
        catch { throw new Error('文件不是有效的 UTF-8 或带 BOM 的 UTF-16 文本，请另存为 UTF-8 后重试。'); }
        loaded.push({ name: file.name, text });
      }
      validateTextAttachments([...existing, ...loaded]);
    } catch (error) { throw fileReadError(file.name, error); }
  }
  validateTextAttachments([...existing, ...loaded]);
  return loaded;
}
