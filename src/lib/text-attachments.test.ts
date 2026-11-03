import { beforeEach, describe, expect, it, vi } from 'vitest';
import { extractDocumentText } from './commands/message';
import { loadTextAttachments, validateTextAttachments, MAX_DOCUMENT_FILE_BYTES, MAX_TEXT_ATTACHMENT_BYTES } from './text-attachments';

vi.mock('./commands/message', () => ({ extractDocumentText: vi.fn() }));
const extractMock = vi.mocked(extractDocumentText);

describe('explicit text attachment snapshots', () => {
  beforeEach(() => vi.resetAllMocks());
  it('reads UTF-8 and BOM without changing content or exposing a path', async () => {
    const text = '标题,待办\r\n用户,处理事项\n';
    expect(await loadTextAttachments([new File(['\uFEFF', text], '安排.CSV')])).toEqual([{ name: '安排.CSV', text }]);
    for (const extension of ['txt', 'md', 'csv', 'tsv', 'json', 'log', 'yaml', 'yml']) {
      expect(await loadTextAttachments([new File(['ordinary text'], `notes.${extension}`)])).toHaveLength(1);
    }
    const text16 = 'Windows 日志\r\n事项：完成\t✓';
    for (const littleEndian of [true, false]) {
      const bytes = utf16(text16, littleEndian);
      expect(await loadTextAttachments([new File([bytes], 'Windows.log')])).toEqual([{ name: 'Windows.log', text: text16 }]);
    }
  });

  it('rejects unsupported, path-like, blank or deceptive filenames before reading', async () => {
    const reader = vi.spyOn(FileReader.prototype, 'readAsArrayBuffer');
    try {
      for (const name of ['photo.png', 'report.pdf', 'book.xls', 'macros.xlsm', 'old.doc', ' .docx', '../notes.txt', 'folder\\notes.txt', 'bad\n.txt', 'bad\u202e.txt', '.txt', 'txt', `${'中'.repeat(67)}.txt`]) {
        const error = await loadTextAttachments([new File(['hello'], name)]).catch((failure: unknown) => failure);
        expect(error).toBeInstanceOf(Error);
        expect((error as Error).message).not.toContain(name);
      }
      expect(reader).not.toHaveBeenCalled();
    } finally { reader.mockRestore(); }
  });

  it('rejects invalid UTF-8, binary controls and empty files visibly instead of decoding lossily', async () => {
    for (const content of [new Uint8Array([0xc3, 0x28]), new Uint8Array([0xff, 0xfe, 65]), new Uint8Array([0xff, 0xfe, 0, 0xd8]), new Uint8Array([0xfe, 0xff, 0xdc, 0]), 'a\0b', 'a\x1bb', 'a\x7fb', ' \n\t', '\uFEFF', utf16('a\0b'), utf16('')]) {
      await expect(loadTextAttachments([new File([content], 'data.txt')])).rejects.toThrow('“data.txt”');
    }
  });

  it('enforces count and UTF-8 byte budgets atomically without truncation', async () => {
    const full = 'a'.repeat(MAX_TEXT_ATTACHMENT_BYTES);
    expect(await loadTextAttachments([new File([full], 'full.txt')])).toEqual([{ name: 'full.txt', text: full }]);
    await expect(loadTextAttachments([new File([`${full}a`], 'large.txt')])).rejects.toThrow('16 KiB');
    await expect(loadTextAttachments([new File(['中'.repeat(5462)], 'unicode.txt')])).rejects.toThrow('16 KiB');
    await expect(loadTextAttachments([new File(['a'], 'third.txt')], [{ name: 'one.txt', text: full }, { name: 'two.txt', text: full }])).rejects.toThrow('32 KiB');
    await expect(loadTextAttachments(Array.from({ length: 5 }, (_, index) => new File(['a'], `${index}.txt`)))).rejects.toThrow('4 个');
    expect(() => validateTextAttachments([{ name: 'data.txt', text: 'a'.repeat(32769) }])).toThrow('16 KiB');
    expect(await loadTextAttachments([new File([utf16(full)], 'Windows.log')])).toEqual([{ name: 'Windows.log', text: full }]);
    expect(await loadTextAttachments([new File([utf16(full)], 'one.txt'), new File([utf16(full, false)], 'two.txt')])).toHaveLength(2);
    await expect(loadTextAttachments([new File([utf16('中'.repeat(5462))], 'unicode.txt')])).rejects.toThrow('16 KiB');
  });

  it('reports a read failure without yielding a partial batch', async () => {
    const reader = vi.spyOn(FileReader.prototype, 'readAsArrayBuffer').mockImplementation(function (this: FileReader) {
      this.dispatchEvent(new ProgressEvent('error'));
    });
    try { await expect(loadTextAttachments([new File(['hello'], 'data.txt')])).rejects.toThrow('无法添加“data.txt”：无法读取'); }
    finally { reader.mockRestore(); }
  });

  it('charges document extracted text, not ZIP bytes, and preserves the same snapshot contract', async () => {
    const bytes = new Uint8Array(MAX_TEXT_ATTACHMENT_BYTES + 1).fill(255);
    for (const format of ['DOCX', 'XLSX']) {
      const snapshot = { name: `安排.${format}`, text: `【${format} 文字快照】\n事项\t完成` };
      extractMock.mockResolvedValueOnce(snapshot);
      expect(await loadTextAttachments([new File([bytes], snapshot.name)])).toEqual([snapshot]);
      expect(extractMock).toHaveBeenLastCalledWith(snapshot.name, btoa(String.fromCharCode(...bytes)));
      expect(Object.keys(snapshot)).toEqual(['name', 'text']);
    }
    const reader = vi.spyOn(FileReader.prototype, 'readAsArrayBuffer');
    await expect(loadTextAttachments([new File([new Uint8Array(MAX_DOCUMENT_FILE_BYTES + 1)], 'large.xlsx')])).rejects.toThrow(/“large.xlsx”.*4 MiB/);
    expect(reader).not.toHaveBeenCalled();
    reader.mockRestore();
  });

  it('rejects failed, mismatched and oversized DOCX extraction atomically without truncation', async () => {
    const files = [new File(['ordinary'], 'notes.txt'), new File(['PK'], 'notes.docx')];
    extractMock.mockRejectedValueOnce(new Error('DOCX 文件损坏'));
    await expect(loadTextAttachments(files)).rejects.toThrow('无法添加“notes.docx”：DOCX 文件损坏');
    extractMock.mockResolvedValueOnce({ name: 'other.docx', text: 'content' });
    await expect(loadTextAttachments(files)).rejects.toThrow('与所选文件不符');
    extractMock.mockResolvedValueOnce({ name: 'notes.docx', text: 'a'.repeat(MAX_TEXT_ATTACHMENT_BYTES + 1) });
    await expect(loadTextAttachments(files)).rejects.toThrow('16 KiB');
    extractMock.mockResolvedValueOnce({ name: 'notes.docx', text: 'x' });
    await expect(loadTextAttachments([files[1]], [
      { name: 'one.txt', text: 'a'.repeat(MAX_TEXT_ATTACHMENT_BYTES) },
      { name: 'two.txt', text: 'a'.repeat(MAX_TEXT_ATTACHMENT_BYTES) },
    ])).rejects.toThrow('32 KiB');
    extractMock.mockRejectedValueOnce({ privateDiagnostic: 'RAW_PRIVATE_CANARY' });
    await expect(loadTextAttachments([new File(['PK'], 'notes.xlsx')])).rejects.toThrow('无法添加“notes.xlsx”：无法读取文件，请重新选择。');
  });

});

function utf16(text: string, littleEndian = true): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(2 + text.length * 2);
  const view = new DataView(bytes.buffer);
  view.setUint16(0, 0xfeff, littleEndian);
  for (let i = 0; i < text.length; i++) view.setUint16(2 + i * 2, text.charCodeAt(i), littleEndian);
  return bytes;
}
