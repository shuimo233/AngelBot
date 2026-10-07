import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

// Vitest deliberately stubs CSS modules; inspect the authored token source.
const tokens = readFileSync('src/ui/tokens.css', 'utf8');
const globalStyles = readFileSync('src/styles/global.css', 'utf8');
const productStyles = readFileSync('src/styles/product.css', 'utf8');

function themeColors(selector: string) {
  const block = tokens.match(new RegExp(`${selector}\\s*\\{([^}]+)\\}`))?.[1] ?? '';
  return Object.fromEntries([...block.matchAll(/(--ui-[\w-]+):\s*(#[a-f\d]{6});/gi)].map((m) => [m[1], m[2]]));
}

function luminance(hex: string) {
  const channels = [1, 3, 5].map((offset) => parseInt(hex.slice(offset, offset + 2), 16) / 255)
    .map((v) => v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrast(a: string, b: string) {
  const values = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (values[0] + 0.05) / (values[1] + 0.05);
}

describe('AngelBot visual foundations', () => {
  it.each([':root', '\\.dark'])('keeps normal, helper, link and action text readable in %s', (selector) => {
    const c = themeColors(selector);
    for (const [foreground, background] of [
      ['--ui-text', '--ui-canvas'], ['--ui-text-secondary', '--ui-surface'],
      ['--ui-muted', '--ui-surface'], ['--ui-accent-text', '--ui-canvas'],
      ['--ui-accent-text-hover', '--ui-canvas'], ['--ui-accent-ink', '--ui-accent'],
      ['--ui-danger', '--ui-surface'],
    ]) expect(contrast(c[foreground], c[background]), `${foreground} on ${background}`).toBeGreaterThanOrEqual(4.5);
    expect(contrast(c['--ui-border-strong'], c['--ui-surface'])).toBeGreaterThanOrEqual(3);
    expect(contrast(c['--ui-focus'], c['--ui-surface'])).toBeGreaterThanOrEqual(3);
  });

  it('has one theme owner, without global wildcard reskinning', () => {
    for (const styles of [globalStyles, productStyles]) {
      expect(styles).not.toMatch(/--color-(?:bg|surface|accent)\s*:/);
      expect(styles).not.toContain("[class*='card']");
    }
    expect(tokens).toContain('--color-accent-fill: var(--ui-accent)');
    expect(tokens).toContain('--color-accent-hover: var(--ui-accent-text-hover)');
  });
});
