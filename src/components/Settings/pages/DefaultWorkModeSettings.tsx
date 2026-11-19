import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import { InlineNotice, RadioChoices } from '../../../ui';
import { ToggleField } from '../components/ToggleField';
import { usePreferencesStore } from '$stores/preferences';
import { useThinkingEffortStore } from '$stores/thinkingEffort';

const responseLengthOptions = [
  { value: 'short', label: '简洁' },
  { value: 'medium', label: '适中' },
  { value: 'long', label: '详细' },
] as const;

const responseLanguageOptions = [
  { value: 'auto', label: '跟随用户语言' },
  { value: 'zh-CN', label: '简体中文' },
  { value: 'en-US', label: 'English' },
] as const;

const thinkingOptions = [
  { value: 'low', label: '快速' },
  { value: 'medium', label: '平衡' },
  { value: 'high', label: '深入' },
] as const;

export function DefaultWorkModeSettings() {
  const preferences = usePreferencesStore((state) => state.preferences);
  const updatePreferences = usePreferencesStore((state) => state.updatePreferences);
  const persistPreferences = usePreferencesStore((state) => state.persistPreferences);
  const setMemoryControls = usePreferencesStore((state) => state.setMemoryControls);
  const memoryControlsBusy = usePreferencesStore((state) => state.memoryControlsBusy);
  const memoryControlsError = usePreferencesStore((state) => state.memoryControlsError);
  const effort = useThinkingEffortStore((state) => state.effort);
  const setEffort = useThinkingEffortStore((state) => state.setEffort);
  const updateHabits = (partial: Partial<typeof preferences.habits>) => {
    updatePreferences({ habits: { ...preferences.habits, ...partial } });
    persistPreferences();
  };

  return (
    <div className="settings-page-content default-work-mode-settings">
      <SettingsSection title="对话默认值">
        <div className="settings-rows">
          <SettingsRow title="默认回复长度">
            <RadioChoices label="默认回复长度" value={preferences.habits.responseLength} options={responseLengthOptions}
              onChange={responseLength => updateHabits({ responseLength })} />
          </SettingsRow>
          <SettingsRow title="默认回复语言">
            <RadioChoices label="默认回复语言" value={preferences.habits.responseLanguage} options={responseLanguageOptions}
              onChange={responseLanguage => updateHabits({ responseLanguage })} />
          </SettingsRow>
          <SettingsRow title="默认推理强度" description="模型支持此参数时生效。">
            <RadioChoices label="默认推理强度" value={effort} options={thinkingOptions} onChange={setEffort} />
          </SettingsRow>
          <ToggleField
            label="使用长期记忆"
            description="关闭后，新回合、续接和定时任务停用记忆。已有记忆保留。"
            checked={preferences.habits.useLongTermMemory ?? true}
            disabled={memoryControlsBusy}
            onChange={(useLongTermMemory) => void setMemoryControls({ useLongTermMemory })}
          />
        </div>
        {memoryControlsError && <InlineNotice tone="danger">记忆开关未能同步。{memoryControlsError}</InlineNotice>}
      </SettingsSection>

    </div>
  );
}
