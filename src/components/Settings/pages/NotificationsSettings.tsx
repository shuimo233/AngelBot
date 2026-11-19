import { useEffect, useRef, useState } from 'react';
import { Button, InlineNotice } from '../../../ui';
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
  const [loaded, setLoaded] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [readAttempt, setReadAttempt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const saving = useRef(false);

  useEffect(() => {
    const request = ++generation.current;
    setIsLoading(true);
    setLoaded(false);
    setError(null);

    getNotificationSettings()
      .then((loadedSettings) => {
        if (generation.current !== request) return;
        setSettings(loadedSettings);
        setLoaded(true);
        setError(null);
      })
      .catch(() => {
        if (generation.current !== request) return;
        setError('无法读取通知设置，请重新读取后再修改。');
      })
      .finally(() => {
        if (generation.current === request) {
          setIsLoading(false);
        }
      });

    return () => {
      generation.current += 1;
    };
  }, [readAttempt]);

  const updateChannel = async (
    key: keyof NotificationSettings,
    checked: boolean
  ) => {
    if (!loaded || isLoading || saving.current) return;
    const request = generation.current;
    const next = { ...settings, [key]: checked };
    saving.current = true;
    setIsSaving(true);
    setError(null);
    try {
      await saveNotificationSettings(next);
      if (generation.current === request) setSettings(next);
    } catch {
      if (generation.current === request) {
        setLoaded(false);
        setError('未能确认通知设置已保存。当前显示上次读取的设置，请重新读取核对；不会自动重试保存。');
      }
    } finally {
      saving.current = false;
      if (generation.current === request) setIsSaving(false);
    }
  };
  const disabled = !loaded || isLoading || isSaving;

  return (
    <div className="settings-page-content">
      <div className="settings-rows" role="group" aria-label="通知类型" aria-busy={isLoading || isSaving}>
        <ToggleField
          label="后台回复完成"
          checked={settings.chat_reply}
          disabled={disabled}
          onChange={(checked) => updateChannel('chat_reply', checked)}
        />
        <ToggleField
          label="定时任务提醒"
          checked={settings.reminder}
          disabled={disabled}
          onChange={(checked) => updateChannel('reminder', checked)}
        />
        <ToggleField
          label="任务结果"
          description="包括任务、脚本或 MAA 的失败与停止。"
          checked={settings.task_complete}
          disabled={disabled}
          onChange={(checked) => updateChannel('task_complete', checked)}
        />
        <ToggleField
          label="文件变更"
          description="仅限监视中的工作目录。"
          checked={settings.file_change}
          disabled={disabled}
          onChange={(checked) => updateChannel('file_change', checked)}
        />
        <ToggleField
          label="系统状态"
          checked={settings.system}
          disabled={disabled}
          onChange={(checked) => updateChannel('system', checked)}
        />
      </div>
      {error && <InlineNotice tone="danger" className="settings-action-line">
        <p>{error}</p>
        <Button variant="secondary" disabled={isLoading || isSaving}
          onClick={() => setReadAttempt(value => value + 1)}>重新读取通知设置</Button>
      </InlineNotice>}
      {(isLoading || isSaving) && <InlineNotice>
        {isLoading ? '正在读取通知设置…' : '正在保存通知设置…'}
      </InlineNotice>}
    </div>
  );
}
