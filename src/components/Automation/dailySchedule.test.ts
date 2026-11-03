import { describe, expect, it } from 'vitest';
import { normalizeDailySchedule } from './dailySchedule';

describe('daily scheduler input', () => {
  it.each([['9:00', '每天 09:00'], ['每天 18:30', '每天 18:30'], [' 00:00 ', '每天 00:00']])('normalizes %s without changing the frequency', (input, expected) => {
    expect(normalizeDailySchedule(input)).toBe(expected);
  });
  it.each(['每周一 09:00', '工作日 09:00', '明天 09:00', '24:00', '12:60', '每天 9:00 运行脚本'])('rejects unsupported or ambiguous input: %s', input => {
    expect(normalizeDailySchedule(input)).toBeNull();
  });
});
