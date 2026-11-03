/** Match the existing daily scheduler; never downgrade an unsupported frequency. */
export function normalizeDailySchedule(value: string): string | null {
  const match = value.trim().match(/^(?:每天\s*)?([01]?\d|2[0-3]):([0-5]\d)$/);
  return match ? `每天 ${match[1].padStart(2, '0')}:${match[2]}` : null;
}
