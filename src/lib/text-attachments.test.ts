import { describe, expect, it, vi } from 'vitest';
import { loadTextAttachments, validateTextAttachments, MAX_TEXT_ATTACHMENT_BYTES } from './text-attachments';

describe('explicit text attachment snapshots', () => {
  it('reads UTF-8 and BOM without changing content or exposing a path', async () => {
    const text = '标题,待办\r\n用户,处理事项\n';
    expect(await loadTextAttachments([new File(['\uFEFF', text], '安排.CSV')])).toEqual([{ name: '安排.CSV', text }]);
    for (const extension of ['txt', 'md', 'csv', 'tsv', 'json', 'log', 'yaml', 'yml']) {
      expect(await loadTextAttachments([new File(['ordinary text'], `notes.${extension}`)])).toHaveLength(1);
    }
  });

  it('rejects unsupported, path-like, blank or deceptive filenames before reading', async () => {
    const reader = vi.spyOn(FileReader.prototype, 'readAsArrayBuffer');
    try {
      for (const name of ['photo.png', 'report.pdf', 'book.xlsx', '../notes.txt', 'folder\\notes.txt', 'bad\n.txt', 'bad\u202e.txt', '.txt', 'txt', `${'中'.repeat(67)}.txt`]) {
        await expect(loadTextAttachments([new File(['hello'], name)])).rejects.toThrow();
      }
      expect(reader).not.toHaveBeenCalled();
    } finally { reader.mockRestore(); }
  });

  it('rejects invalid UTF-8, binary controls and empty files visibly instead of decoding lossily', async () => {
    for (const content of [new Uint8Array([0xc3, 0x28]), new Uint8Array([0xff, 0xfe, 65, 0]), 'a\0b', 'a\x1bb', 'a\x7fb', ' \n\t', '\uFEFF']) {
      await expect(loadTextAttachments([new File([content], 'data.txt')])).rejects.toThrow();
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
  });

  it('reports a read failure without yielding a partial batch', async () => {
    const reader = vi.spyOn(FileReader.prototype, 'readAsArrayBuffer').mockImplementation(function (this: FileReader) {
      this.dispatchEvent(new ProgressEvent('error'));
    });
    try { await expect(loadTextAttachments([new File(['hello'], 'data.txt')])).rejects.toThrow('无法读取'); }
    finally { reader.mockRestore(); }
  });
});
