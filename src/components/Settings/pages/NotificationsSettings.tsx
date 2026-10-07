import { useEffect, useState } from 'react';
import { SettingsSection } from '../components/SettingsSection';
import { ToggleField } from '../components/ToggleField';
import {
  getNotificationSettings,
  saveNotificationSettings,
  type NotificationSettings,
} from '$lib/commands/settings';

const DEFAULT_NOTIFICATION_SETTINGS: NotificationSettings = {
  chat_reply: true,
  reminder: true,
  task_complete: true,
  file_change: true,
  system: true,
};

export function NotificationsSettings() {
  const [settings, setSettings] = useState<NotificationSettings>(
    DEFAULT_NOTIFICATION_SETTINGS
  );
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;

    getNotificationSettings()
      .then((loadedSettings) => {
        if (!active) return;
        setSettings(loadedSettings);
        setError(null);
      })
      .catch(() => {
        if (!active) return;
        setError('Failed to load notification settings');
      })
      .finally(() => {
        if (active) {
          setIsLoading(false);
        }
      });

    return () => {
      active = false;
    };
  }, []);

  const updateChannel = async (
    key: keyof NotificationSettings,
    checked: boolean
  ) => {
    const previous = settings;
    const next = { ...settings, [key]: checked };

    setSettings(next);
    try {
      await saveNotificationSettings(next);
      setError(null);
    } catch {
      setSettings(previous);
      setError('Failed to save notification settings');
    }
  };

  return (
    <div className="settings-page-content">
      {error && <div className="settings-error">{error}</div>}

      <SettingsSection
        title="通知策略"
        description="只开启你希望被打断时接收的通知。设置会立即保存并应用。"
        defaultOpen
      >
        <ToggleField
          label="后台回复完成"
          description="当 AngelBot 在后台完成回复时通知你。"
          checked={settings.chat_reply}
          onChange={(checked) => updateChannel('chat_reply', checked)}
        />
        <ToggleField
          label="定时任务提醒"
          description="当已创建的定时任务需要处理时通知你。"
          checked={settings.reminder}
          onChange={(checked) => updateChannel('reminder', checked)}
        />
        <ToggleField
          label="任务完成"
          description="当长时间运行的任务完成时通知你。"
          checked={settings.task_complete}
          onChange={(checked) => updateChannel('task_complete', checked)}
        />
        <ToggleField
          label="文件变更"
          description="当受监视的工作目录发生变更时通知你。"
          checked={settings.file_change}
          onChange={(checked) => updateChannel('file_change', checked)}
        />
        <ToggleField
          label="系统状态"
          description="允许应用健康状态和重要错误通知。"
          checked={settings.system}
          onChange={(checked) => updateChannel('system', checked)}
        />
        {isLoading && <span className="settings-loading">正在读取通知设置…</span>}
      </SettingsSection>
    </div>
  );
}
