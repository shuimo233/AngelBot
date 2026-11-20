import { describe, expect, it } from 'vitest';
import { appendTavernNotes, applyTavernCard, cardDescription, exportTavernCard, parseTavernCard, parseTavernCardFile, parseTavernCardPng, readTavernBook } from './tavern-card';

const v2 = {
  spec: 'chara_card_v2', spec_version: '2.0', data: {
    name: 'Mira', description: 'A patient archivist.', personality: 'Thoughtful and warm.',
    scenario: 'A quiet library.', first_mes: 'Welcome back.', mes_example: '<START>\n{{char}}: Hello.',
    system_prompt: 'Stay in character.', post_history_instructions: 'Keep answers concise.',
    alternate_greetings: ['Good evening.'], creator_notes: 'Test card', creator: 'Author', tags: ['library'], extensions: { source: 'test' },
    character_book: { entries: [{ keys: ['library'], content: 'Preserved lore, not a runtime engine.' }] },
  },
};

const v3 = { spec: 'chara_card_v3', spec_version: '3.0', data: { ...v2.data,
  name: '弥生 V3', description: '{{char}} 是温柔的助手。🌙', character_version: '1.2',
  nickname: '弥生', source: ['https://example.invalid/source'], group_only_greetings: ['大家好'],
  creator_notes_multilingual: { zh: '中文说明', en: 'Notes' }, creation_date: 1, modification_date: 2,
  assets: [{ type: 'icon', uri: 'https://example.invalid/do-not-load.png', name: 'main', ext: 'png' }],
  avatar: 'https://example.invalid/not-a-runtime-avatar.png',
  character_book: { name: 'V3 资料', extensions: {}, entries: [
    { keys: ['图书馆'], content: '中文参考资料', enabled: true, insertion_order: 0, extensions: {}, use_regex: false },
    { keys: ['conditional'], content: '@@exclude_keys private\n不可忽略此条件', enabled: true, constant: true, insertion_order: 1, extensions: {}, use_regex: false },
  ] },
} };

const basePng = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGP4DwABAQEAsTj2FAAAAABJRU5ErkJggg=='), char => char.charCodeAt(0));
const encodeCard = (value: unknown) => btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(value))));

/** Valid 1x1 PNG, with CRC-checked text chunks inserted after IHDR. */
function cardPng(metadata: { keyword: string; payload: string }[]): ArrayBuffer {
  const chunks = metadata.map(({ keyword, payload }) => {
    const data = new TextEncoder().encode(`${keyword}\0${payload}`);
    const chunk = new Uint8Array(data.length + 12);
    const view = new DataView(chunk.buffer);
    view.setUint32(0, data.length);
    chunk.set(new TextEncoder().encode('tEXt'), 4);
    chunk.set(data, 8);
    let crc = 0xffffffff;
    for (const byte of chunk.subarray(4, 8 + data.length)) {
      crc ^= byte;
      for (let bit = 0; bit < 8; bit++) crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
    view.setUint32(8 + data.length, (crc ^ 0xffffffff) >>> 0);
    return chunk;
  });
  const result = new Uint8Array(basePng.length + chunks.reduce((sum, chunk) => sum + chunk.length, 0));
  result.set(basePng.subarray(0, 33));
  let offset = 33;
  for (const chunk of chunks) { result.set(chunk, offset); offset += chunk.length; }
  result.set(basePng.subarray(33), offset);
  return result.buffer;
}

describe('TavernCard compatibility', () => {
  it('previews native book text without executing its conditions or automatically applying it', () => {
    const book = { name: '资料', entries: [
      { keys: ['/regex/i'], content: '虚构图书馆资料', use_regex: true, extensions: { probability: 50 } },
      { content: '常驻', constant: true }, { content: '关闭', enabled: false },
    ] };
    const card = parseTavernCard({ ...v2, data: { ...v2.data, character_book: book } });
    const preview = readTavernBook(card.card)!;
    expect(preview).toMatchObject({ name: '资料', totalEntries: 3, omittedEntries: 0 });
    expect(preview.entries).toHaveLength(3);
    expect(applyTavernCard(card).personaNotes).toBe('');
    expect(appendTavernNotes('已有说明', [preview.entries[0]])).toBe('已有说明\n\n条目 1：\n虚构图书馆资料');
    expect(card.card.rawData?.character_book).toEqual(book);
    expect(JSON.stringify(exportTavernCard({ ...applyTavernCard(card), personaNotes: 'USER_EDITED_NOTES' } as import('$types').Persona))).not.toContain('USER_EDITED_NOTES');
  });

  it('bounds previews and manual notes without truncating or changing the original book', () => {
    const book = { entries: Array.from({ length: 258 }, () => ({ content: 'entry' })) };
    const card = parseTavernCard({ ...v2, data: { ...v2.data, character_book: book } }).card;
    expect(readTavernBook(card)).toMatchObject({ totalEntries: 258, omittedEntries: 2 });
    expect(readTavernBook(card)?.entries).toHaveLength(256);
    expect(card.rawData?.character_book).toEqual(book);
    expect(readTavernBook()).toBeUndefined();
    expect(() => appendTavernNotes('x'.repeat(4095), [{ sourceId: '0', label: 'long', content: 'body' }])).toThrow(/4096/);
    expect(appendTavernNotes('x'.repeat(4096), [])).toHaveLength(4096);
  });

  it('maps a V2 card without dropping its roleplay fields', () => {
    const card = parseTavernCard(v2);
    expect(applyTavernCard(card)).toMatchObject({ name: 'Mira', bio: 'A patient archivist.', greeting: 'Welcome back.' });
    expect(cardDescription(card)).toContain('A quiet library.');
    expect(cardDescription(card)).toContain('A patient archivist.');
    expect(cardDescription(card)).toContain('Thoughtful and warm.');
  });

  it('excludes systemPrompt and postHistoryInstructions from cardDescription to prevent model-identity pollution', () => {
    const card = parseTavernCard(v2);
    const desc = cardDescription(card);
    // system_prompt and post_history_instructions should NOT be injected into
    // the persona description — they would put model-specific identities (e.g.
    // "You are Claude") into the prompt, causing other models to adopt the
    // wrong identity.
    expect(desc).not.toContain('Stay in character.');
    expect(desc).not.toContain('Keep answers concise.');
  });

  it.each([
    { source: 'V1', value: v2.data },
    { source: 'V2', value: v2 },
    { source: 'legacy V1 export', value: { ...v2, spec_version: '1.0' } },
  ])('exports $source as V2 2.0 and preserves extension and lore data', ({ value }) => {
    const parsed = parseTavernCard(value);
    if ('spec' in value) expect(parsed.card.specVersion).toBe('2.0');
    const result = exportTavernCard({ ...applyTavernCard(parsed), languageStyle: 'casual', tone: 'friendly', responseFormats: ['text'], keywords: [], personality: 'balanced', speechBubble: 'default' } as import('$types').Persona);
    expect(result).toMatchObject({ spec: 'chara_card_v2', spec_version: '2.0', data: {
      name: 'Mira', extensions: { source: 'test' }, character_book: v2.data.character_book,
    } });
    expect(parseTavernCard(result).card.rawData).toMatchObject({ extensions: { source: 'test' }, character_book: v2.data.character_book });
  });

  it('reads UTF-8 chara metadata without corrupting Chinese or emoji', () => {
    const unicodeCard = { ...v2, data: { ...v2.data, name: '弥生助手', description: '温柔、耐心。🌙', first_mes: '欢迎回来。' } };
    const parsed = parseTavernCardPng(cardPng([{ keyword: 'chara', payload: encodeCard(unicodeCard) }]));
    expect(parsed).toMatchObject({ name: '弥生助手', description: '温柔、耐心。🌙', firstMes: '欢迎回来。' });
    expect(parsed.card.rawData).toMatchObject({ extensions: v2.data.extensions, character_book: v2.data.character_book });
  });

  it.each([
    { label: 'unknown spec', value: { ...v2.data, spec: 'future_card' }, message: /不支持.*规范/ },
    { label: 'wrong V2 version', value: { ...v2, spec_version: '3.0' }, message: /版本/ },
    { label: 'malformed V2 data', value: { ...v2, name: 'Not a flat V1 fallback', data: [] }, message: /data/ },
    { label: 'unknown V3 version', value: { ...v3, spec_version: '3.1' }, message: /V3.*版本/ },
    { label: 'missing V3 version', value: { ...v3, spec_version: undefined }, message: /V3.*版本/ },
    { label: 'malformed V3 data', value: { ...v3, data: [] }, message: /V3.*data/ },
  ])('rejects $label explicitly instead of treating it as V1 or reporting a missing name', ({ value, message }) => {
    expect(() => parseTavernCard(value)).toThrow(message);
  });

  const chara = { keyword: 'chara', payload: encodeCard(v2) };
  const ccv3 = { keyword: 'ccv3', payload: encodeCard(v3) };
  it.each([
    { label: 'V3 only', metadata: [ccv3] },
    { label: 'V2 before V3', metadata: [chara, ccv3] },
    { label: 'V3 before V2', metadata: [ccv3, chara] },
  ])('reads $label PNG as V3 without downgrading to its chara layer', ({ metadata }) => {
    const parsed = parseTavernCardPng(cardPng(metadata));
    expect(parsed).toMatchObject({ name: '弥生 V3', description: '{{char}} 是温柔的助手。🌙', card: { specVersion: '3.0' } });
    expect(parsed.avatar).toBeUndefined();
    expect(parsed.card.rawData).toEqual(v3.data);
  });

  it('uses the same V3 JSON parser for chara metadata, preserves extras on reopen/export and excludes non-text runtime fields', () => {
    const parsed = parseTavernCardPng(cardPng([{ keyword: 'chara', payload: ccv3.payload }]));
    expect(parsed.card.specVersion).toBe('3.0');
    const profile = JSON.parse(JSON.stringify(applyTavernCard(parsed)));
    const exported = exportTavernCard(profile);
    expect(exported).toMatchObject({ spec: 'chara_card_v3', spec_version: '3.0', data: v3.data });
    expect(parseTavernCard(exported).card.rawData).toEqual(v3.data);
    expect(cardDescription(parsed)).not.toContain(v3.data.system_prompt);
    expect(cardDescription(parsed)).not.toContain(v3.data.post_history_instructions);
    expect(readTavernBook(parsed.card)).toMatchObject({ totalEntries: 2, omittedEntries: 0 });
    expect(profile.personaNotes).toBe('');
  });

  it.each(['!!!', btoa('{"name":"\xff"}'), encodeCard({ ...v3, spec_version: '4.0' }), encodeCard(v2)])('rejects invalid preferred ccv3 data instead of falling back to a valid chara', payload => {
    expect(() => parseTavernCardPng(cardPng([chara, { keyword: 'ccv3', payload }]))).toThrow(/无法读取|V3/);
  });

  it.each([
    { value: { name: 'Flat V1', character_book: { entries: [{ keys: ['key'], content: 'text' }] } }, spec: 'chara_card_v2', version: '2.0' },
    { value: { spec: 'chara_card_v2', spec_version: '2.0', data: { name: 'V2' } }, spec: 'chara_card_v2', version: '2.0' },
    { value: { spec: 'chara_card_v3', spec_version: '3.0', data: { name: 'V3' } }, spec: 'chara_card_v3', version: '3.0' },
  ])('fills required export defaults for $version without upgrading V1/V2 to V3', ({ value, spec, version }) => {
    const exported = exportTavernCard(applyTavernCard(parseTavernCard(value)) as import('$types').Persona);
    expect(exported).toMatchObject({ spec, spec_version: version, data: {
      description: '', personality: '', scenario: '', first_mes: '', mes_example: '',
      system_prompt: '', post_history_instructions: '', alternate_greetings: [],
      creator_notes: '', creator: '', character_version: '', tags: [], extensions: {},
    } });
    if ('character_book' in (exported.data as Record<string, unknown>)) {
      expect(exported).toMatchObject({ data: { character_book: { extensions: {}, entries: [{ keys: ['key'], content: 'text', enabled: true, insertion_order: 0, extensions: {} }] } } });
    }
    if (version === '3.0') expect(exported).toMatchObject({ data: { group_only_greetings: [] } });
  });

  it('rejects CHARX clearly without attempting to load or extract assets', async () => {
    await expect(parseTavernCardFile(new File(['not a JSON document'], 'card.charx'))).rejects.toThrow(/CHARX/);
  });

  it.each(['!!!', btoa('{"name":"\xff"}')])('rejects undecodable metadata without accepting corrupt text', payload => {
    expect(() => parseTavernCardPng(cardPng([{ keyword: 'chara', payload }]))).toThrow(/数据无法读取/);
  });

  it('retains PNG signature and truncated-chunk bounds checks', () => {
    expect(() => parseTavernCardPng(new ArrayBuffer(7))).toThrow(/PNG/);
    expect(() => parseTavernCardPng(cardPng([chara]).slice(0, 40))).toThrow(/角色卡数据/);
  });
});
