import { useRef, useState } from 'react';
import { SettingsSection } from '../components/SettingsSection';
import { SelectField } from '../components/SelectField';
import { ToggleField } from '../components/ToggleField';
import { useThemeStore } from '$stores/theme';
import { useAppearanceStore } from '$stores/appearance';
import { Button } from '../../../ui';

const themeOptions = [
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
  { value: 'system', label: '跟随系统' },
];

export function AppearanceSettings() {
  const theme = useThemeStore((state) => state.theme);
  const setTheme = useThemeStore((state) => state.setTheme);
  const fontSize = useAppearanceStore((state) => state.chatFontSize);
  const reduceMotion = useAppearanceStore((state) => state.reduceMotion);
  const bgImage = useAppearanceStore((state) => state.bgImage);
  const bgOpacity = useAppearanceStore((state) => state.bgOpacity);
  const persistenceError = useAppearanceStore((state) => state.persistenceError);
  const update = useAppearanceStore((state) => state.update);
  const fileInput = useRef<HTMLInputElement>(null);
  const [imageError, setImageError] = useState('');

  const handleBgImageUpload = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    if (!['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(file.type) || file.size > 2 * 1024 * 1024) {
      setImageError('请选择不超过 2 MB 的 PNG、JPG、WebP 或 GIF 图片。');
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      update({ bgImage: String(reader.result) });
      setImageError('');
    };
    reader.onerror = () => setImageError('无法读取图片，请重新选择。');
    reader.readAsDataURL(file);
  };

  return (
    <div className="settings-page-content appearance-settings">
      {persistenceError && <p className="ui-notice ui-notice--warning" role="status">外观已应用，但本机存储不可用或空间不足，重启后可能无法保留。</p>}
      <SettingsSection title="主题" description="颜色主题与侧栏快捷切换保持同步。" defaultOpen>
        <SelectField label="颜色主题" value={theme} options={themeOptions} onChange={(value) => {
          if (value === 'light' || value === 'dark' || value === 'system') setTheme(value);
        }} />
      </SettingsSection>
      <SettingsSection title="阅读与动效" description="调整对话文字，不改变工具栏和导航的大小。" defaultOpen>
        <div className="slider-field">
          <label htmlFor="chat-font-size">对话文字大小</label>
          <div className="slider-row">
            <input id="chat-font-size" type="range" min="12" max="20" value={fontSize}
              onChange={(event) => update({ chatFontSize: Number(event.target.value) })} />
            <output className="slider-value" htmlFor="chat-font-size">{fontSize}px</output>
          </div>
        </div>
        <p className="appearance-reading-preview" aria-label="对话文字预览" style={{ fontSize }}>
          把要处理的事情告诉我，我们一起理清下一步。
        </p>
        <ToggleField label="减少动态效果" description="关闭非必要的过渡和位移动画；始终尊重系统的减少动态效果设置。"
          checked={reduceMotion} onChange={(value) => update({ reduceMotion: value })} />
      </SettingsSection>
      <SettingsSection title="背景图片" description="仅在本机保存，用于聊天区域。支持 PNG、JPG、WebP 或 GIF，最大 2 MB。">
        {bgImage && <img src={bgImage} alt="背景预览" className="bg-preview-img" />}
        <div className="ui-inline">
          <input ref={fileInput} type="file" accept="image/png,image/jpeg,image/webp,image/gif" onChange={handleBgImageUpload} hidden />
          <Button onClick={() => fileInput.current?.click()}>{bgImage ? '更换背景图片' : '选择背景图片'}</Button>
          {bgImage && <Button variant="ghost" onClick={() => { update({ bgImage: '' }); setImageError(''); }}>移除背景</Button>}
        </div>
        {imageError && <p role="alert" className="ui-notice ui-notice--danger">{imageError}</p>}
        {bgImage && <div className="slider-field">
          <label htmlFor="bg-opacity">背景不透明度</label>
          <div className="slider-row">
            <input id="bg-opacity" type="range" min="0" max="100" value={bgOpacity}
              onChange={(event) => update({ bgOpacity: Number(event.target.value) })} />
            <output className="slider-value" htmlFor="bg-opacity">{bgOpacity}%</output>
          </div>
        </div>}
      </SettingsSection>
    </div>
  );
}
