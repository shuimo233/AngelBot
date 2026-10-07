import { describe, expect, it } from 'vitest';
import { parseAutomationIntent, titleFromPrompt } from './parseAutomationIntent';

describe('parseAutomationIntent', () => {
  it('解析「每天晚上九点提醒我整理当天笔记」', () => {
    const result = parseAutomationIntent('每天晚上九点提醒我整理当天笔记');
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.intent.prompt).toBe('整理当天笔记');
    expect(result.intent.frequency).toBe('每天');
    expect(result.intent.time).toBe('21:00');
    expect(result.intent.triggerValue).toBe('每天 21:00');
    expect(result.intent.executorKind).toBe('agent');
  });

  it('解析 24 小时制时间「每天 21:00 提醒我喝水」', () => {
    const result = parseAutomationIntent('每天 21:00 提醒我喝水');
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.intent.time).toBe('21:00');
    expect(result.intent.prompt).toBe('喝水');
  });

  it('拒绝未支持的每周频率而不是将它当作每日重复', () => {
    const result = parseAutomationIntent('每周一 9:30 提醒我写周报');
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('重复自动化目前只支持每天');
  });

  it('拒绝未支持的工作日频率而不是将它当作每日重复', () => {
    const result = parseAutomationIntent('工作日早上八点半提醒我站会');
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('重复自动化目前只支持每天');
  });

  it('将明天的一次性提醒明确引导到主对话', () => {
    const result = parseAutomationIntent('明天下午三点提醒我打电话');
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('一次性提醒请在对话中创建');
  });

  it('中午按 12 小时制处理「每天中午十二点半提醒我吃饭」', () => {
    const result = parseAutomationIntent('每天中午十二点半提醒我吃饭');
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.intent.time).toBe('12:30');
  });

  it('保留动作里的明天信息，不将每日任务误判为一次性提醒', () => {
    const result = parseAutomationIntent('每天09:00提醒我整理明天会议的材料');
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.intent.triggerValue).toBe('每天 09:00');
    expect(result.intent.prompt).toBe('整理明天会议的材料');
  });

  it.each(['每天工作日09:00提醒我站会', '每天每周一09:00提醒我写周报'])('拒绝时间前的矛盾调度前缀：%s', (description) => {
    const result = parseAutomationIntent(description);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('多个重复频率');
  });

  it('缺少频率时要求明确重复意图，不擅自每天提醒', () => {
    const result = parseAutomationIntent('9点30分提醒我吃药');
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('请明确每日重复频率');
  });

  it('识别「运行脚本」并切换为 script 执行方式', () => {
    const result = parseAutomationIntent('每天晚上九点半运行脚本 scripts/backup.py 备份当天笔记');
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.intent.executorKind).toBe('script');
    expect(result.intent.scriptPath).toBe('scripts/backup.py');
    expect(result.intent.time).toBe('21:30');
    expect(result.intent.prompt).toBe('备份当天笔记');
  });

  it('识别「记得」前缀「每天晚上十点记得写日记」', () => {
    const result = parseAutomationIntent('每天晚上十点记得写日记');
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.intent.prompt).toBe('写日记');
  });

  it('没有时间时给出具体提示', () => {
    const result = parseAutomationIntent('每天提醒我整理笔记');
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('时间');
  });

  it('没有动作内容时给出具体提示', () => {
    const result = parseAutomationIntent('每天九点提醒我');
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('要做什么');
  });

  it('空输入时报错而不是静默失败', () => {
    expect(parseAutomationIntent('   ').ok).toBe(false);
  });
});

describe('titleFromPrompt', () => {
  it('短文本原样返回', () => {
    expect(titleFromPrompt('整理当天笔记')).toBe('整理当天笔记');
  });

  it('超过 20 字截断并加省略号', () => {
    const long = '这是一段非常非常非常长的动作描述文本内容啊';
    expect(titleFromPrompt(long)).toBe('这是一段非常非常非常长的动作描述文本内容…');
  });
});
