import { useState } from 'react';
import { SettingsSection } from '../components/SettingsSection';
import { SelectField } from '../components/SelectField';
import { ToggleField } from '../components/ToggleField';
import { useThemeStore } from '$stores/theme';

const themeOptions = [
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
  { value: 'system', label: '跟随系统' },
];

const langOptions = [
  { value: 'zh-CN', label: '中文' },
  { value: 'en', label: 'English' },
];

export function AppearanceSettings() {
  const theme = useThemeStore((state) => state.theme);
  const setTheme = useThemeStore((state) => state.setTheme);
  const [language, setLanguage] = useState('zh-CN');
  const [fontSize, setFontSize] = useState(14);
  const [animations, setAnimations] = useState(true);
  const [bgImage, setBgImage] = useState(() => localStorage.getItem('bgImage') || '');
  const [bgOpacity, setBgOpacity] = useState(() => Number(localStorage.getItem('bgOpacity') || 30));

  const handleThemeChange = (value: string) => {
    if (value === 'light' || value === 'dark' || value === 'system') setTheme(value);
  };

  const handleBgImageUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => {
      const dataUrl = ev.target?.result as string;
      setBgImage(dataUrl);
      localStorage.setItem('bgImage', dataUrl);
      applyBgImage(dataUrl, bgOpacity);
    };
    reader.readAsDataURL(file);
  };

  const applyBgImage = (url: string, opacity: number) => {
    const root = document.documentElement;
    root.style.setProperty('--bg-image', url ? `url("${url}")` : 'none');
    root.style.setProperty('--bg-opacity', String(opacity / 100));
  };

  const handleBgOpacityChange = (value: number) => {
    setBgOpacity(value);
    localStorage.setItem('bgOpacity', String(value));
    applyBgImage(bgImage, value);
  };

  const handleRemoveBgImage = () => {
    setBgImage('');
    localStorage.removeItem('bgImage');
    document.documentElement.style.removeProperty('--bg-image');
  };

  // 初始化背景图片
  if (bgImage && !document.documentElement.style.getPropertyValue('--bg-image')) {
    applyBgImage(bgImage, bgOpacity);
  }

  return (
    <div className="settings-page-content">
      <SettingsSection title="主题" description="选择应用的外观主题">
        <SelectField
          label="颜色主题"
          value={theme}
          options={themeOptions}
          onChange={handleThemeChange}
        />
      </SettingsSection>

      <SettingsSection title="背景图片" description="设置聊天区域的背景图片">
        {bgImage ? (
          <div className="bg-preview">
            <img src={bgImage} alt="背景预览" className="bg-preview-img" />
            <div className="bg-preview-actions">
              <button className="btn btn-secondary btn-sm" onClick={handleRemoveBgImage}>移除</button>
            </div>
          </div>
        ) : (
          <div className="bg-upload-area">
            <label className="bg-upload-btn">
              <input type="file" accept="image/*" onChange={handleBgImageUpload} hidden />
              <span>上传背景图片</span>
            </label>
          </div>
        )}
        {bgImage && (
          <div className="slider-field" style={{ marginTop: 12 }}>
            <label>背景透明度</label>
            <div className="slider-row">
              <input
                type="range"
                min="0"
                max="100"
                value={bgOpacity}
                onChange={(e) => handleBgOpacityChange(Number(e.target.value))}
              />
              <span className="slider-value">{bgOpacity}%</span>
            </div>
          </div>
        )}
      </SettingsSection>

      <SettingsSection title="语言" description="选择界面显示语言">
        <SelectField
          label="界面语言"
          value={language}
          options={langOptions}
          onChange={setLanguage}
        />
      </SettingsSection>

      <SettingsSection title="显示" description="调整界面显示效果">
        <div className="slider-field">
          <label>字体大小</label>
          <div className="slider-row">
            <input
              type="range"
              min="12"
              max="20"
              value={fontSize}
              onChange={(e) => setFontSize(Number(e.target.value))}
            />
            <span className="slider-value">{fontSize}px</span>
          </div>
        </div>
        <ToggleField
          label="界面动画"
          description="页面切换和元素动画效果"
          checked={animations}
          onChange={setAnimations}
        />
      </SettingsSection>
    </div>
  );
}
