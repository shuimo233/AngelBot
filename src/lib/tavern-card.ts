import type { Persona, TavernCharacterCard } from '$types';

type JsonObject = Record<string, unknown>;

export type ParsedTavernCard = {
  name: string;
  description: string;
  firstMes: string;
  avatar?: string;
  card: TavernCharacterCard;
};

const text = (value: unknown) => typeof value === 'string' ? value : '';
const strings = (value: unknown) => Array.isArray(value)
  ? value.filter((item): item is string => typeof item === 'string')
  : [];

function object(value: unknown): JsonObject | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as JsonObject
    : null;
}

/** V1/V2/V3 text-persona subset. Assets and high-priority card instructions never run. */
export function parseTavernCard(value: unknown): ParsedTavernCard {
  const root = object(value);
  if (!root) throw new Error('角色卡不是有效的 JSON 对象。');

  if (root.spec !== undefined && root.spec !== 'chara_card_v2' && root.spec !== 'chara_card_v3') {
    throw new Error('不支持此角色卡规范，当前支持 V1、V2 和 V3 的文本人设与世界书子集。');
  }
  const isV2 = root.spec === 'chara_card_v2';
  const isV3 = root.spec === 'chara_card_v3';
  if (isV3 && root.spec_version !== '3.0') {
    throw new Error('暂不支持此 V3 角色卡版本，当前仅支持 spec_version 3.0 的文本子集。');
  }
  // Older AngelBot exports wrapped V1-origin data in a V2 envelope with a 1.0 header.
  // Read that known legacy output as V2; V1/V2 exports normalize to 2.0.
  if (isV2 && root.spec_version !== undefined && root.spec_version !== '2.0' && root.spec_version !== '1.0') {
    throw new Error('暂不支持此角色卡版本，请导出为 TavernCard V2（spec_version 2.0）后导入。');
  }
  const data = isV2 || isV3 ? object(root.data) : root;
  if (!data) throw new Error(`TavernCard ${isV3 ? 'V3' : 'V2'} 的 data 必须是 JSON 对象。`);
  const name = text(data.name).trim();
  if (!name) throw new Error('角色卡缺少角色名称。');

  const description = text(data.description);
  const personality = text(data.personality);
  const scenario = text(data.scenario);
  const card: TavernCharacterCard = {
    specVersion: isV3 ? '3.0' : isV2 ? '2.0' : '1.0',
    personality,
    scenario,
    mesExample: text(data.mes_example),
    systemPrompt: text(data.system_prompt),
    postHistoryInstructions: text(data.post_history_instructions),
    alternateGreetings: strings(data.alternate_greetings),
    creatorNotes: text(data.creator_notes),
    creator: text(data.creator),
    tags: strings(data.tags),
    rawData: { ...data },
  };

  return { name, description, firstMes: text(data.first_mes), avatar: isV3 ? undefined : text(data.avatar) || undefined, card };
}

/** Strict UTF-8 text metadata; ccv3 takes precedence over chara in either order. */
export function parseTavernCardPng(bytes: ArrayBuffer): ParsedTavernCard {
  const view = new DataView(bytes);
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  if (view.byteLength < signature.length || signature.some((byte, index) => view.getUint8(index) !== byte)) {
    throw new Error('不是有效的 PNG 角色卡。');
  }
  const decoder = new TextDecoder('latin1');
  let encodedCard: string | undefined;
  let encodedV3: string | undefined;
  let offset = 8;
  while (offset + 12 <= view.byteLength) {
    const length = view.getUint32(offset);
    const type = decoder.decode(new Uint8Array(bytes, offset + 4, 4));
    const dataStart = offset + 8;
    const next = dataStart + length + 4;
    if (next > view.byteLength) throw new Error('PNG 中的角色卡数据不完整。');
    if (type === 'tEXt' || type === 'iTXt' || type === 'zTXt') {
      // Inspect only the PNG keyword; embedded asset bytes are never decoded.
      const header = new Uint8Array(bytes, dataStart, Math.min(length, 80));
      const separator = header.indexOf(0);
      const keyword = separator === -1 ? '' : decoder.decode(header.subarray(0, separator));
      if (keyword === 'ccv3' && type !== 'tEXt') {
        throw new Error('PNG 的 V3 ccv3 数据不是支持的 tEXt 格式。');
      }
      if (type === 'tEXt' && (keyword === 'chara' || keyword === 'ccv3')) {
        const payload = decoder.decode(new Uint8Array(bytes, dataStart + separator + 1, length - separator - 1));
        if (keyword === 'ccv3') encodedV3 ??= payload;
        else encodedCard ??= payload;
      }
    }
    offset = next;
  }
  const encoded = encodedV3 ?? encodedCard;
  if (encoded === undefined) throw new Error('PNG 中没有找到 SillyTavern 角色卡数据。');
  let value: unknown;
  try {
    const payload = Uint8Array.from(atob(encoded), char => char.charCodeAt(0));
    value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(payload));
  } catch {
    throw new Error('PNG 中的角色卡数据无法读取。');
  }
  const parsed = parseTavernCard(value);
  if (encodedV3 !== undefined && parsed.card.specVersion !== '3.0') {
    throw new Error('PNG 的 ccv3 数据不是有效的 V3 3.0 角色卡。');
  }
  return parsed;
}

export async function parseTavernCardFile(file: File): Promise<ParsedTavernCard> {
  if (file.name.toLowerCase().endsWith('.charx')) {
    throw new Error('暂不支持 CHARX 和嵌入资产，请导入 JSON 或 PNG 文本角色卡。');
  }
  if (file.name.toLowerCase().endsWith('.png') || file.type === 'image/png') {
    return parseTavernCardPng(await file.arrayBuffer());
  }
  return parseTavernCard(JSON.parse(await file.text()));
}

export function cardDescription(card: ParsedTavernCard): string {
  return [
    card.description && `角色描述：\n${card.description}`,
    card.card.personality && `性格：\n${card.card.personality}`,
    card.card.scenario && `场景：\n${card.card.scenario}`,
    // NOTE: systemPrompt and postHistoryInstructions are deliberately excluded.
    // Injecting them here puts model-specific identities (e.g. "You are Claude")
    // directly into the persona description, causing DeepSeek and other models to
    // adopt the wrong identity. Retaining them for export grants no runtime authority.
  ].filter(Boolean).join('\n\n');
}

export const PERSONA_NOTES_LIMIT = 4096;

export type TavernBookEntry = { sourceId: string; label: string; content: string };
export type TavernBookPreview = {
  name: string;
  entries: TavernBookEntry[];
  totalEntries: number;
  omittedEntries: number;
};

/** Read-only source text for manual selection. No ST activation rules execute. */
export function readTavernBook(card?: TavernCharacterCard): TavernBookPreview | undefined {
  const book = object(card?.rawData?.character_book);
  if (!book || !Array.isArray(book.entries)) return undefined;
  const entries: TavernBookEntry[] = [];
  let remaining = 256 * 1024;
  for (const [index, value] of book.entries.slice(0, 256).entries()) {
    const entry = object(value);
    const content = text(entry?.content);
    if (!entry || !content.trim() || content.length > 16384 || content.length > remaining) continue;
    remaining -= content.length;
    entries.push({ sourceId: `character_book.entries.${index}`,
      label: (text(entry.name) || text(entry.comment) || `条目 ${index + 1}`).slice(0, 128), content });
  }
  return { name: (text(book.name) || '世界书').slice(0, 128), entries,
    totalEntries: book.entries.length, omittedEntries: book.entries.length - entries.length };
}

export function appendTavernNotes(current: string, selected: TavernBookEntry[]): string {
  const result = [current.trim(), ...selected.map(entry => `${entry.label}：\n${entry.content}`)].filter(Boolean).join('\n\n');
  if (result.length > PERSONA_NOTES_LIMIT) throw new Error('人设补充最多 4096 字；请减少所选条目，或复制需要的部分并自行编辑。');
  return result;
}

export function applyTavernCard(card: ParsedTavernCard): Partial<Persona> {
  return {
    name: card.name,
    avatar: card.avatar || '',
    bio: card.description,
    greeting: card.firstMes,
    characterCard: card.card,
    personaNotes: '',
  };
}

export function exportTavernCard(profile: Persona, description = profile.bio): JsonObject {
  const card = profile.characterCard;
  const isV3 = card?.specVersion === '3.0';
  const raw = card?.rawData ?? {};
  const data: JsonObject = {
    ...raw,
    name: profile.name || 'AngelBot',
    description,
    personality: card?.personality ?? '',
    scenario: card?.scenario ?? '',
    first_mes: profile.greeting || '',
    mes_example: card?.mesExample ?? '',
    creator_notes: card?.creatorNotes ?? '',
    system_prompt: card?.systemPrompt ?? '',
    post_history_instructions: card?.postHistoryInstructions ?? '',
    alternate_greetings: card?.alternateGreetings ?? [],
    creator: card?.creator ?? 'AngelBot',
    tags: card?.tags ?? [],
    character_version: raw.character_version === undefined ? '' : raw.character_version,
    extensions: raw.extensions === undefined ? {} : raw.extensions,
  };
  const book = object(raw.character_book);
  if (book) {
    const entries = Array.isArray(book.entries) ? book.entries.map((value, index) => {
      const entry = object(value);
      if (!entry) return value;
      // Fill only absent fields. Repairing a malformed rule could activate it.
      return { keys: [], content: '', enabled: true, insertion_order: index, extensions: {},
        ...(isV3 ? { use_regex: false } : {}), ...entry };
    }) : book.entries === undefined ? [] : book.entries;
    data.character_book = { extensions: {}, ...book, entries };
  }
  if (isV3) data.group_only_greetings = raw.group_only_greetings === undefined ? [] : raw.group_only_greetings;
  return { spec: isV3 ? 'chara_card_v3' : 'chara_card_v2', spec_version: isV3 ? '3.0' : '2.0', data };
}
