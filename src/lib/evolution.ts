import type { UserPreferences } from '$types';

/**
 * 展示用户明确配置的偏好，不冒充后台学习结果。
 */
export function describeUserPreferences(preferences: UserPreferences): string {
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
    return '尚未设定额外偏好。';
  }

  return parts.join('\n');
}
