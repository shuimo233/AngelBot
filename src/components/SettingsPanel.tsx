import { useEffect } from 'react';
import { useSettingsStore } from '$stores/settings';

export function SettingsPanel() {
  const profile = useSettingsStore((state) => state.profile);
  const loadProfile = useSettingsStore((state) => state.loadProfile);
  const updateProfile = useSettingsStore((state) => state.updateProfile);
  const persistProfile = useSettingsStore((state) => state.persistProfile);

  useEffect(() => { loadProfile(); }, [loadProfile]);

  const handleNameChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    updateProfile({ name: e.target.value });
  };

  const handleBioChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    updateProfile({ bio: e.target.value });
  };

  return (
    <aside className="settings-panel">
      <div className="settings-title">设置</div>
      <div className="settings-block">
        <label className="field">
          <span>人设名称</span>
          <input
            value={profile.name}
            onChange={handleNameChange}
            placeholder="AI 伴侣名称"
          />
        </label>
        <label className="field">
          <span>简介</span>
          <textarea
            value={profile.bio}
            onChange={handleBioChange}
            placeholder="输入简介..."
            rows={3}
          />
        </label>
        <button className="primary" onClick={persistProfile}>保存</button>
      </div>
    </aside>
  );
}
