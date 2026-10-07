/**
 * 自动化自然语言创建：纯前端解析，不调用 LLM。
 *
 * 支持句式示例：
 * - 每天晚上九点提醒我整理当天笔记
 * - 每天 21:00 提醒我喝水
 * - 每天晚上九点半运行脚本 scripts/backup.py 备份笔记
 */

export interface AutomationIntent {
  /** 去掉「提醒我/记得」等前缀后的动作文本 */
  prompt: string;
  /** 当前重复自动化仅支持每天。 */
  frequency: '每天';
  /** HH:MM（24 小时制） */
  time: string;
  executorKind: 'agent' | 'script';
  scriptPath?: string;
  /** 传给后端的每日触发文本：每天 HH:MM。 */
  triggerValue: string;
}

export type ParseAutomationResult =
  | { ok: true; intent: AutomationIntent }
  | { ok: false; error: string };

/** 与现有每日调度契约一致，不从额外文字中挑出时间后降级创建。 */
export function normalizeDailySchedule(value: string): string | null {
  const match = value.trim().match(/^(?:每天\s*)?([01]?\d|2[0-3]):([0-5]\d)$/);
  return match ? `每天 ${match[1].padStart(2, '0')}:${match[2]}` : null;
}

const CN_DIGITS: Record<string, number> = {
  零: 0, 一: 1, 二: 2, 两: 2, 三: 3, 四: 4, 五: 5, 六: 6, 七: 7, 八: 8, 九: 9,
};

/** 解析 0-59 的中文/阿拉伯数字（支持「十」「十五」「二十五」「零五」「半」由调用方处理）。 */
function parseChineseNumber(text: string): number | null {
  if (!text) return null;
  if (/^\d+$/.test(text)) return Number(text);
  const tenIndex = text.indexOf('十');
  if (tenIndex === -1) {
    if (text.length === 1 && text in CN_DIGITS) return CN_DIGITS[text];
    if (text.length === 2 && text[0] === '零' && text[1] in CN_DIGITS) return CN_DIGITS[text[1]];
    return null;
  }
  const tensPart = text.slice(0, tenIndex);
  const onesPart = text.slice(tenIndex + 1);
  const tens = tensPart ? CN_DIGITS[tensPart] : 1;
  if (tens === undefined || tens === null) return null;
  let ones = 0;
  if (onesPart) {
    if (!(onesPart in CN_DIGITS)) return null;
    ones = CN_DIGITS[onesPart];
  }
  return tens * 10 + ones;
}

const PERIOD = '(凌晨|早上|早晨|上午|中午|下午|傍晚|晚上|晚间|夜里)';
const PM_PERIODS = new Set(['中午', '下午', '傍晚', '晚上', '晚间', '夜里']);

const COLON_TIME_RE = new RegExp(`${PERIOD}?\\s*(\\d{1,2})\\s*[:：]\\s*(\\d{1,2})`);
const DOT_TIME_RE = new RegExp(
  `${PERIOD}?\\s*(\\d{1,2}|[零一二两三四五六七八九十]{1,4})\\s*点\\s*(半|(\\d{1,2}|[零一二两三四五六七八九十]{1,4})\\s*分?)?`,
);

interface TimeMatch {
  text: string;
  time: string;
}

function formatTime(hour: number, minute: number, period?: string): string | null {
  if (minute < 0 || minute > 59) return null;
  if (period && PM_PERIODS.has(period) && hour <= 12) {
    if (hour < 1) return null;
    if (hour < 12) hour += 12;
  } else {
    if (period === '凌晨' && hour === 12) hour = 0;
    if (hour < 0 || hour > 23) return null;
  }
  return `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`;
}

/** 从文本中提取时间表达，返回原始片段与 HH:MM；未识别时返回 null。 */
function extractTime(text: string): TimeMatch | null {
  const colon = text.match(COLON_TIME_RE);
  if (colon) {
    const hour = Number(colon[2]);
    const minute = Number(colon[3]);
    const time = formatTime(hour, minute, colon[1]);
    if (time) return { text: colon[0], time };
  }
  const dot = text.match(DOT_TIME_RE);
  if (dot) {
    const hour = parseChineseNumber(dot[2]);
    if (hour === null) return null;
    let minute = 0;
    const minutePart = dot[3];
    if (minutePart === '半') {
      minute = 30;
    } else if (minutePart) {
      const digits = minutePart.match(/^\d+/)?.[0];
      const parsed = digits ? Number(digits) : parseChineseNumber(minutePart.replace(/\s*分$/, ''));
      if (parsed === null) return null;
      minute = parsed;
    }
    const time = formatTime(hour, minute, dot[1]);
    if (time) return { text: dot[0], time };
  }
  return null;
}

const FREQUENCY_RE = /每天|每日|天天|(?:每(?:个)?)?工作日|每(?:周|星期)[一二两三四五六日天]?|明天|明早|明晚|今天|今晚|一次性/;

const SCRIPT_RE = /运行脚本[：:]?\s*([^\s，。、；;]+)/;

const ACTION_PREFIX_RE = /^(请|帮我|麻烦|记得要|记得|提醒我|提醒|叫我|别忘了|别忘记|不要忘记)/;

/** 去掉动作文本里的引导词与首尾标点。 */
function cleanActionText(text: string): string {
  let result = text.replace(/\s+/g, ' ').trim();
  result = result.replace(/^[\s，。,.、；;：:！!？?]+/, '');
  let previous = '';
  while (previous !== result) {
    previous = result;
    result = result.replace(ACTION_PREFIX_RE, '').replace(/^[\s，。,.、；;：:！!？?]+/, '');
  }
  result = result.replace(/[\s，。,.、；;：:！!？?]+$/, '');
  result = result.replace(/一下$/, '').replace(/[\s，。,.、；;：:！!？?]+$/, '');
  return result.trim();
}

export function parseAutomationIntent(input: string): ParseAutomationResult {
  let rest = input.trim();
  if (!rest) {
    return { ok: false, error: '请先输入一句话描述，例如「每天晚上九点提醒我整理当天笔记」。' };
  }

  let executorKind: 'agent' | 'script' = 'agent';
  let scriptPath: string | undefined;
  const scriptMatch = rest.match(SCRIPT_RE);
  if (scriptMatch) {
    executorKind = 'script';
    scriptPath = scriptMatch[1];
    rest = rest.replace(scriptMatch[0], ' ');
  }

  const frequency = rest.match(FREQUENCY_RE)?.[0];
  if (!frequency) {
    return { ok: false, error: '请明确每日重复频率，如「每天 21:00」；一次性提醒请在对话中创建。' };
  }
  if (/^(明天|明早|明晚|今天|今晚|一次性)$/.test(frequency)) {
    return { ok: false, error: '一次性提醒请在对话中创建，告诉 AngelBot 具体日期、时间和提醒内容；不会改成每日任务。' };
  }
  if (!/^(每天|每日|天天)$/.test(frequency)) {
    return { ok: false, error: '重复自动化目前只支持每天，不支持每周或工作日；不会改成每日任务。' };
  }
  rest = rest.replace(frequency, ' ');

  const timeMatch = extractTime(rest);
  if (!timeMatch) {
    return { ok: false, error: '没有识别出时间，请补充具体时间，如「每天 21:00」。' };
  }
  // The first frequency was already consumed. Reject another only in the
  // timing prefix; dates inside the action are not scheduling instructions.
  if (FREQUENCY_RE.test(rest.slice(0, rest.indexOf(timeMatch.text)))) {
    return { ok: false, error: '时间前包含多个重复频率，请只保留一个「每天」；不会自动选择或改成每日任务。' };
  }
  rest = rest.replace(timeMatch.text, ' ');

  const prompt = cleanActionText(rest);
  if (!prompt) {
    return { ok: false, error: '没有识别出要做什么，请补充具体内容，如「提醒我整理当天笔记」。' };
  }

  return {
    ok: true,
    intent: {
      prompt,
      frequency: '每天',
      time: timeMatch.time,
      executorKind,
      scriptPath,
      triggerValue: `每天 ${timeMatch.time}`,
    },
  };
}

/** 列表里显示的任务名：取动作文本前 20 字。 */
export function titleFromPrompt(prompt: string): string {
  const characters = Array.from(prompt.trim());
  return characters.length > 20 ? `${characters.slice(0, 20).join('')}…` : characters.join('');
}
