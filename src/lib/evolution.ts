import type { PersonalityTemplate, TraitConfig, UserPreferences } from '$types';
import { TRAIT_LABELS } from '$lib/presets';

/**
 * 将 trait 配置转换为自然语言描述
 */
export function traitsToDescription(traits: TraitConfig): string {
  const parts: string[] = [];

  for (const [key, value] of Object.entries(traits)) {
    if (value === 0) continue;
    
    const traitKey = key as keyof TraitConfig;
    const labels = TRAIT_LABELS[traitKey];
    
    if (!labels) continue;

    const label = value > 0 ? labels.right : labels.left;
    const absValue = Math.abs(value);

    if (absValue >= 4) {
      parts.push(`非常${label}`);
    } else if (absValue >= 2) {
      parts.push(`${label}`);
    } else {
      parts.push(`稍微${label}`);
    }
  }

  return parts.length > 0 ? parts.join('，') : '中性';
}

/**
 * 将模板转换为系统提示词
 */
export function templateToSystemPrompt(template: PersonalityTemplate): string {
  const parts: string[] = [];

  if (template.description) {
    parts.push(`角色设定：${template.description}`);
  }

  const traitsDesc = traitsToDescription(template.traits);
  parts.push(`性格特征：${traitsDesc}`);

  if (template.greeting) {
    parts.push(`开场白：${template.greeting}`);
  }

  return parts.join('\n');
}

/**
 * 根据用户偏好调整提示词
 */
export function applyUserPreferences(
  systemPrompt: string,
  preferences: UserPreferences
): string {
  if (!preferences.evolutionEnabled) {
    return systemPrompt;
  }

  const additions: string[] = [];

  additions.push('使用自然、克制的语言，不使用 emoji、颜文字或拟人化装饰。');

  if (preferences.habits.responseLength === 'short') {
    additions.push('回复尽量简洁。');
  } else if (preferences.habits.responseLength === 'long') {
    additions.push('回复可以详细一些。');
  }

  if (preferences.topics.interests.length > 0) {
    additions.push(`用户感兴趣的话题：${preferences.topics.interests.join('、')}。`);
  }

  if (preferences.topics.avoidTopics.length > 0) {
    additions.push(`请避免谈论：${preferences.topics.avoidTopics.join('、')}。`);
  }

  if (preferences.communication.dislikedWords.length > 0) {
    additions.push(`请避免使用：${preferences.communication.dislikedWords.join('、')}。`);
  }

  if (preferences.communication.petPeeves.length > 0) {
    additions.push(`用户的雷点：${preferences.communication.petPeeves.join('、')}。`);
  }

  if (additions.length > 0) {
    return `${systemPrompt}\n\n用户偏好：\n${additions.join('\n')}`;
  }

  return systemPrompt;
}

/**
 * 从对话中提取用户偏好（简化版）
 */
export function extractPreferencesFromConversation(
  messages: { role: 'user' | 'assistant'; content: string }[]
): Partial<UserPreferences> {
  const preferences: Partial<UserPreferences> = {};

  // 简化实现：检测用户消息的长度偏好
  const userMessages = messages.filter((m) => m.role === 'user');
  if (userMessages.length > 0) {
    const avgLength =
      userMessages.reduce((sum, m) => sum + m.content.length, 0) / userMessages.length;

    if (avgLength < 30) {
      preferences.habits = { ...preferences.habits!, responseLength: 'short' };
    } else if (avgLength > 200) {
      preferences.habits = { ...preferences.habits!, responseLength: 'long' };
    }
  }

  return preferences;
}

/**
 * 生成自进化摘要
 */
export function generateEvolutionSummary(preferences: UserPreferences): string {
  const parts: string[] = [];

  if (preferences.habits.responseLength !== 'medium') {
    parts.push(`回复长度偏好：${preferences.habits.responseLength === 'short' ? '简短' : '详细'}`);
  }

  if (preferences.topics.interests.length > 0) {
    parts.push(`感兴趣的话题：${preferences.topics.interests.join('、')}`);
  }

  if (preferences.topics.avoidTopics.length > 0) {
    parts.push(`想避免的话题：${preferences.topics.avoidTopics.join('、')}`);
  }

  if (preferences.communication.dislikedWords.length > 0) {
    parts.push(`不喜欢的用词：${preferences.communication.dislikedWords.join('、')}`);
  }

  if (parts.length === 0) {
    return '还没有学习到新的偏好，继续和我聊天吧~';
  }

  return `我学到了以下关于你的信息：\n${parts.join('\n')}`;
}
