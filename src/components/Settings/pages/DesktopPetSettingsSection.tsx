import { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DESKTOP_PET_CHANGED, getDesktopPet, resetDesktopPetPosition, setDesktopPet, type DesktopPetStatus } from '$lib/commands/desktop-pet';
import { isTauriRuntime } from '$lib/invoke';
import mascot from '../../../../artwork/mascot/sd/default-transparent.png';
import { Button } from '../../../ui';
import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import { ToggleField } from '../components/ToggleField';
import './DesktopPetSettingsSection.css';

const UNAVAILABLE: DesktopPetStatus = { supported: false, enabled: false };

export function DesktopPetSettingsSection() {
  const native = isTauriRuntime();
  const [status, setStatus] = useState<DesktopPetStatus | null>(native ? null : UNAVAILABLE);
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const revision = useRef(0);
  const changing = useRef(false);

  useEffect(() => {
    if (!native) return;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    const refresh = () => {
      if (changing.current) return;
      const started = ++revision.current;
      void getDesktopPet().then(value => {
        if (!disposed && revision.current === started) setStatus(value);
      }).catch(() => {
        if (!disposed && revision.current === started) setError('无法读取桌宠状态，请重新打开设置后重试。');
      });
    };
    refresh();
    window.addEventListener('focus', refresh);
    void listen<DesktopPetStatus>(DESKTOP_PET_CHANGED, ({ payload }) => {
      if (disposed) return;
      revision.current += 1;
      setStatus(payload);
      setError('');
      setNotice('');
    }).then(stop => {
      if (disposed) stop();
      else {
        unlisten = stop;
        refresh();
      }
    }).catch(() => {
      // Window focus can still refresh status if event registration is unavailable.
    });
    return () => {
      disposed = true;
      window.removeEventListener('focus', refresh);
      unlisten?.();
    };
  }, [native]);

  const change = async (enabled: boolean) => {
    changing.current = true;
    setPending(true);
    setError('');
    setNotice('');
    const started = ++revision.current;
    try {
      const updated = await setDesktopPet(enabled);
      if (revision.current === started) setStatus(updated);
    } catch {
      setError('无法更改桌宠状态，请重试。');
    } finally {
      changing.current = false;
      setPending(false);
    }
  };

  const reset = async () => {
    setPending(true);
    setError('');
    setNotice('');
    try {
      await resetDesktopPetPosition();
      setNotice('已恢复默认位置。');
    } catch {
      setError('无法重置桌宠位置，请重试。');
    } finally {
      setPending(false);
    }
  };

  return <SettingsSection title="桌宠">
    <div className="settings-rows">
      <div className="desktop-pet-settings-preview">
        <img src={mascot} alt="AngelBot 桌宠预览" width={52} height={64} draggable={false} />
        <ToggleField label="显示桌宠" description={status?.supported ? '拖动移动，点角色打开对话，点爱心摸摸头。' : '在 Windows 桌面版使用。'}
          checked={status?.enabled ?? false} disabled={!status?.supported || pending} onChange={change} />
      </div>
      {status?.enabled && <SettingsRow title="桌宠位置">
        <Button variant="secondary" disabled={pending} onClick={() => void reset()}>恢复默认位置</Button>
      </SettingsRow>}
    </div>
    {notice && <p role="status" className="ui-notice">{notice}</p>}
    {error && <p role="alert" className="ui-notice ui-notice--danger">{error}</p>}
  </SettingsSection>;
}
