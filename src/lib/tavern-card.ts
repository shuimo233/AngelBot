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

/** Parses TavernCard V2 and the still-common flat V1 JSON representation. */
export function parseTavernCard(value: unknown): ParsedTavernCard {
  const root = object(value);
  if (!root) throw new Error('角色卡不是有效的 JSON 对象。');

  const isV2 = root.spec === 'chara_card_v2' && object(root.data);
  const data = (isV2 ? object(root.data) : root)!;
  const name = text(data.name).trim();
  if (!name) throw new Error('角色卡缺少角色名称。');

  const description = text(data.description);
  const personality = text(data.personality);
  const scenario = text(data.scenario);
  const card: TavernCharacterCard = {
    specVersion: isV2 ? text(root.spec_version) || '2.0' : '1.0',
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

  return { name, description, firstMes: text(data.first_mes), avatar: text(data.avatar) || undefined, card };
}

/** Decodes the `chara` tEXt metadata chunk used by SillyTavern PNG character cards. */
export function parseTavernCardPng(bytes: ArrayBuffer): ParsedTavernCard {
  const view = new DataView(bytes);
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  if (view.byteLength < signature.length || signature.some((byte, index) => view.getUint8(index) !== byte)) {
    throw new Error('不是有效的 PNG 角色卡。');
  }
  const decoder = new TextDecoder('latin1');
  let offset = 8;
  while (offset + 12 <= view.byteLength) {
    const length = view.getUint32(offset);
    const type = decoder.decode(new Uint8Array(bytes, offset + 4, 4));
    const dataStart = offset + 8;
    const next = dataStart + length + 4;
    if (next > view.byteLength) break;
    if (type === 'tEXt') {
      const chunk = decoder.decode(new Uint8Array(bytes, dataStart, length));
      const separator = chunk.indexOf('\0');
      if (separator !== -1 && chunk.slice(0, separator) === 'chara') {
        try {
          return parseTavernCard(JSON.parse(atob(chunk.slice(separator + 1))));
        } catch {
          throw new Error('PNG 中的角色卡数据无法读取。');
        }
      }
    }
    offset = next;
  }
  throw new Error('PNG 中没有找到 SillyTavern 角色卡数据。');
}

export async function parseTavernCardFile(file: File): Promise<ParsedTavernCard> {
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
    // adopt the wrong identity. They should be handled as separate system-level
    // instructions at the provider/framework layer instead.
  ].filter(Boolean).join('\n\n');
}

export function applyTavernCard(card: ParsedTavernCard): Partial<Persona> {
  return {
    name: card.name,
    avatar: card.avatar || '',
    bio: card.description,
    greeting: card.firstMes,
    characterCard: card.card,
  };
}

export function exportTavernCard(profile: Persona, description = profile.bio): JsonObject {
  const card = profile.characterCard;
  const data: JsonObject = {
    ...(card?.rawData ?? {}),
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
  };
  return { spec: 'chara_card_v2', spec_version: card?.specVersion || '2.0', data };
}
