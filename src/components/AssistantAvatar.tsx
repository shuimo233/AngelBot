import { useState } from 'react';
import { useSettingsStore } from '$stores/settings';
import defaultAvatar from '../../src-tauri/icons/128x128.png';
import './AssistantAvatar.css';

export function useActiveAssistantName() {
  return useSettingsStore((state) => state.activeProfile?.name.trim() || 'AngelBot');
}

export function AssistantAvatar({ small = false }: { small?: boolean }) {
  const avatar = useSettingsStore((state) => state.activeProfile?.avatar);
  const [failed, setFailed] = useState<string>();
  // Imported cards can contain arbitrary URLs. Only saved, embedded raster
  // images are rendered; opening a chat never fetches a remote avatar.
  const source = avatar && avatar !== failed && avatar.length <= 4 * 1024 * 1024
    && /^data:image\/(?:png|jpeg|webp|gif);base64,[A-Za-z0-9+/]+={0,2}$/i.test(avatar)
    ? avatar : defaultAvatar;
  return <img className={`assistant-avatar${small ? ' assistant-avatar--small' : ''}`}
    src={source} alt="" draggable={false} onError={() => { if (source !== defaultAvatar) setFailed(source); }} />;
}
