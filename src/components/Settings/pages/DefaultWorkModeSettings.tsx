import { SettingsSection } from '../components/SettingsSection';
import { SelectField } from '../components/SelectField';
import { ToggleField } from '../components/ToggleField';
import { usePreferencesStore } from '$stores/preferences';
import { useThinkingEffortStore, type ThinkingEffort } from '$stores/thinkingEffort';

const responseLengthOptions = [
  { value: 'short', label: '简洁' },
  { value: 'medium', label: '适中' },
  { value: 'long', label: '详细' },
];

const responseLanguageOptions = [
  { value: 'auto', label: '跟随用户语言' },
  { value: 'zh-CN', label: '简体中文' },
  { value: 'en-US', label: 'English' },
];

const thinkingOptions = [
  { value: 'low', label: '快速' },
  { value: 'medium', label: '平衡' },
  { value: 'high', label: '深入' },
];

export function DefaultWorkModeSettings() {
  const preferences = usePreferencesStore((state) => state.preferences);
  const updatePreferences = usePreferencesStore((state) => state.updatePreferences);
  const persistPreferences = usePreferencesStore((state) => state.persistPreferences);
  const effort = useThinkingEffortStore((state) => state.effort);
  const setEffort = useThinkingEffortStore((state) => state.setEffort);
  const updateHabits = (partial: Partial<typeof preferences.habits>) => {
    updatePreferences({ habits: { ...preferences.habits, ...partial } });
    persistPreferences();
  };

  return (
    <div className="settings-page-content default-work-mode-settings">
      <SettingsSection
        title="回复方式"
        description="这些偏好会应用到后续回复；在会话中给出的明确要求优先。"
        defaultOpen
      >
        <div className="settings-form-grid">
          <SelectField
            label="默认回复长度"
            value={preferences.habits.responseLength}
            options={responseLengthOptions}
            onChange={(value) => updateHabits({ responseLength: value as 'short' | 'medium' | 'long' })}
          />
          <SelectField
            label="默认回复语言"
            value={preferences.habits.responseLanguage}
            options={responseLanguageOptions}
            onChange={(value) => updateHabits({ responseLanguage: value as 'auto' | 'zh-CN' | 'en-US' })}
          />
        </div>
        <p className="settings-muted">回复采用自然、克制的语言；不使用 Emoji、颜文字或拟人化装饰。</p>
        <ToggleField
          label="使用长期记忆"
          description="关闭后，AngelBot 不会在后续回复中读取长期记忆；不会删除已有记忆。"
          checked={preferences.habits.useLongTermMemory ?? true}
          onChange={(useLongTermMemory) => updateHabits({ useLongTermMemory })}
        />
      </SettingsSection>

      <SettingsSection
        title="推理方式"
        description="作为后续回复的默认推理强度；模型不支持时会自动忽略。"
        defaultOpen
      >
        <SelectField
          label="默认推理强度"
          value={effort}
          options={thinkingOptions}
          onChange={(value) => setEffort(value as ThinkingEffort)}
        />
      </SettingsSection>
    </div>
  );
}
