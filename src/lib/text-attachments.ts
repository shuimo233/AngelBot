import type { TextAttachment } from '$types';

// Mirrored by the backend: these are UTF-8 byte limits, not character limits.
export const MAX_TEXT_ATTACHMENTS = 4;
export const MAX_TEXT_ATTACHMENT_BYTES = 16 * 1024;
export const MAX_TEXT_ATTACHMENTS_BYTES = 32 * 1024;
export const TEXT_ATTACHMENT_EXTENSIONS = ['txt', 'md', 'csv', 'tsv', 'json', 'log', 'yaml', 'yml'];
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
    throw new Error('目前只支持 TXT、Markdown、CSV、TSV、JSON、LOG 和 YAML 文本文件；图片、PDF 和 Office 文件暂不支持。');
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

/** Atomic bounded batch: no paths, lossy decoding, partial selection or truncation. */
export async function loadTextAttachments(files: File[], existing: TextAttachment[] = []): Promise<TextAttachment[]> {
  if (existing.length + files.length > MAX_TEXT_ATTACHMENTS) throw new Error('一次最多添加 4 个文本文件。');
  let size = existing.reduce((sum, file) => sum + textAttachmentBytes(file.text), 0);
  for (const file of files) {
    validateName(file.name);
    if (file.size > MAX_TEXT_ATTACHMENT_BYTES) throw new Error('每个文本文件最多 16 KiB，不会截断内容。');
    size += file.size;
  }
  if (size > MAX_TEXT_ATTACHMENTS_BYTES) throw new Error('附件合计最多 32 KiB，不会截断内容。');
  const loaded: TextAttachment[] = [];
  for (const file of files) {
    const bytes = await readFile(file);
    if (bytes.byteLength > MAX_TEXT_ATTACHMENT_BYTES) throw new Error('每个文本文件最多 16 KiB，不会截断内容。');
    let text: string;
    try { text = new TextDecoder('utf-8', { fatal: true }).decode(bytes).replace(/^\uFEFF/u, ''); }
    catch { throw new Error('文件不是有效的 UTF-8 文本，请另存为 UTF-8 后重试。'); }
    loaded.push({ name: file.name, text });
  }
  validateTextAttachments([...existing, ...loaded]);
  return loaded;
}
