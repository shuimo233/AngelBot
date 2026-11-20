/** Match the existing daily scheduler; never downgrade an unsupported frequency. */
export function normalizeDailySchedule(value: string): string | null {
  const match = value.trim().match(/^(?:每天\s*)?([01]?\d|2[0-3]):([0-5]\d)$/);
  return match ? `每天 ${match[1].padStart(2, '0')}:${match[2]}` : null;
}

export function previewNextDailyRun(value: string, now = new Date()): Date | null {
  const normalized = normalizeDailySchedule(value);
  if (!normalized) return null;
  const [hour, minute] = normalized.slice(-5).split(':').map(Number);
  for (let offset = 0; offset < 3; offset++) {
    const runAt = new Date(now.getFullYear(), now.getMonth(), now.getDate() + offset, hour, minute);
    if (runAt > now && runAt.getHours() === hour && runAt.getMinutes() === minute) return runAt;
  }
  return null;
}
