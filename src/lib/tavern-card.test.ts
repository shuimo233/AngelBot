import { describe, expect, it } from 'vitest';
import { applyTavernCard, cardDescription, exportTavernCard, parseTavernCard, parseTavernCardPng } from './tavern-card';

const v2 = {
  spec: 'chara_card_v2', spec_version: '2.0', data: {
    name: 'Mira', description: 'A patient archivist.', personality: 'Thoughtful and warm.',
    scenario: 'A quiet library.', first_mes: 'Welcome back.', mes_example: '<START>\n{{char}}: Hello.',
    system_prompt: 'Stay in character.', post_history_instructions: 'Keep answers concise.',
    alternate_greetings: ['Good evening.'], creator_notes: 'Test card', creator: 'Author', tags: ['library'], extensions: { source: 'test' },
  },
};

describe('TavernCard compatibility', () => {
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

  it('exports a V2 card and preserves extension data', () => {
    const parsed = parseTavernCard(v2);
    const result = exportTavernCard({ ...applyTavernCard(parsed), languageStyle: 'casual', tone: 'friendly', responseFormats: ['text'], keywords: [], personality: 'balanced', speechBubble: 'default' } as import('$types').Persona);
    expect(result).toMatchObject({ spec: 'chara_card_v2', data: { name: 'Mira', extensions: { source: 'test' } } });
  });

  it('reads a chara PNG text chunk', () => {
    const payload = btoa(JSON.stringify(v2));
    const chunk = new TextEncoder().encode(`chara\0${payload}`);
    const bytes = new Uint8Array(8 + 12 + chunk.length);
    bytes.set([137, 80, 78, 71, 13, 10, 26, 10]);
    new DataView(bytes.buffer).setUint32(8, chunk.length);
    bytes.set(new TextEncoder().encode('tEXt'), 12);
    bytes.set(chunk, 16);
    expect(parseTavernCardPng(bytes.buffer).name).toBe('Mira');
  });
});
