import { useEffect, useId, useRef, useState } from 'react';
import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import { ToggleField } from '../components/ToggleField';
import { useThemeStore } from '$stores/theme';
import { CHAT_TYPOGRAPHY, useAppearanceStore } from '$stores/appearance';
import { Button, RadioChoices, TextField } from '../../../ui';
import { DesktopPetSettingsSection } from './DesktopPetSettingsSection';

const BACKGROUND_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];
const BACKGROUND_IMAGE_MAX_MB = 3.5;
const BACKGROUND_IMAGE_FORMAT = `不超过 ${BACKGROUND_IMAGE_MAX_MB} MB 的 PNG、JPG、WebP 或 GIF`;

const themeOptions = [
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
  { value: 'system', label: '跟随系统' },
] as const;

const fontSuggestions = [
  { value: 'Microsoft YaHei', label: '微软雅黑' },
  { value: 'DengXian', label: '等线' },
  { value: 'SimSun', label: '宋体' },
  { value: 'KaiTi', label: '楷体' },
  { value: 'Source Han Sans SC', label: '思源黑体' },
];

export function AppearanceSettings() {
  const layout = useThemeStore((state) => state.layout);
  const setLayout = useThemeStore((state) => state.setLayout);
  const theme = useThemeStore((state) => state.theme);
  const setTheme = useThemeStore((state) => state.setTheme);
  const fontSize = useAppearanceStore((state) => state.chatFontSize);
  const fontFamily = useAppearanceStore((state) => state.chatFontFamily);
  const reduceMotion = useAppearanceStore((state) => state.reduceMotion);
  const bgImage = useAppearanceStore((state) => state.bgImage);
  const bgOpacity = useAppearanceStore((state) => state.bgOpacity);
  const persistenceError = useAppearanceStore((state) => state.persistenceError);
  const update = useAppearanceStore((state) => state.update);
  const fileInput = useRef<HTMLInputElement>(null);
  const fontListId = useId();
  const [imageError, setImageError] = useState('');
  const imageRead = useRef<FileReader | null>(null);
  useEffect(() => () => { imageRead.current?.abort(); imageRead.current = null; }, []);

  const handleBgImageUpload = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    imageRead.current?.abort();
    imageRead.current = null;
    if (!BACKGROUND_IMAGE_TYPES.includes(file.type) || file.size > BACKGROUND_IMAGE_MAX_MB * 1024 * 1024) {
      setImageError(`请选择${BACKGROUND_IMAGE_FORMAT} 图片。`);
      return;
    }
    setImageError('');
    const reader = new FileReader();
    imageRead.current = reader;
    reader.onload = async () => {
      if (imageRead.current !== reader) return;
      const dataUrl = String(reader.result);
      const image = new Image();
      image.src = dataUrl;
      const decodes = await image.decode().then(() => true, () => false);
      if (imageRead.current !== reader) return;
      imageRead.current = null;
      if (!decodes) {
        setImageError('无法打开这张图片，请重新选择。');
        return;
      }
      update({ bgImage: dataUrl });
    };
    reader.onerror = () => {
      if (imageRead.current !== reader) return;
      imageRead.current = null;
      setImageError('无法读取图片，请重新选择。');
    };
    reader.readAsDataURL(file);
  };

  return (
    <div className="settings-page-content appearance-settings">
      {persistenceError && <p className="ui-notice ui-notice--warning" role="status">外观已应用，但本机存储不可用或空间不足，重启后可能无法保留。</p>}
      <SettingsSection title="界面">
        <div className="settings-rows">
          <SettingsRow title="布局">
            <RadioChoices label="布局" value={layout} options={[{ value: 'A', label: '文字侧栏' }, { value: 'B', label: '图标侧栏' }]} onChange={setLayout} />
          </SettingsRow>
          <SettingsRow title="颜色主题">
            <RadioChoices label="颜色主题" value={theme} options={themeOptions} onChange={setTheme} />
          </SettingsRow>
          <ToggleField label="减少动态效果" description="也会遵循系统的动效设置。"
            checked={reduceMotion} onChange={(value) => update({ reduceMotion: value })} />
        </div>
      </SettingsSection>
      <SettingsSection title="对话文字">
        <datalist id={fontListId}>
          {fontSuggestions.map(({ value, label }) => <option key={value} value={value} label={label} />)}
        </datalist>
        <div className="settings-rows">
          <TextField label="对话字体" value={fontFamily} list={fontListId} maxLength={CHAT_TYPOGRAPHY.maxFamilyLength}
            placeholder="默认字体"
            hint="输入本机字体名称，留空使用默认字体。"
            onChange={(event) => update({ chatFontFamily: event.target.value })} />
          <div className="slider-field">
            <label htmlFor="chat-font-size">对话文字大小</label>
            <div className="slider-row">
              <input id="chat-font-size" type="range" min={CHAT_TYPOGRAPHY.minSize} max={CHAT_TYPOGRAPHY.maxSize} value={fontSize}
                onChange={(event) => update({ chatFontSize: Number(event.target.value) })} />
              <output className="slider-value" htmlFor="chat-font-size">{fontSize}px</output>
            </div>
          </div>
          <SettingsRow title="阅读预览" description={<p className="appearance-reading-preview" aria-label="对话文字预览">
            把要处理的事情告诉我，我们一起理清下一步。AngelBot 123
          </p>}>
            <Button variant="ghost" disabled={!fontFamily && fontSize === CHAT_TYPOGRAPHY.defaultSize}
              onClick={() => update({ chatFontSize: CHAT_TYPOGRAPHY.defaultSize, chatFontFamily: '' })}>恢复默认字体和字号</Button>
          </SettingsRow>
        </div>
      </SettingsSection>
      <SettingsSection title="聊天背景">
        <div className="settings-rows">
          <SettingsRow title="背景图片" description={`支持${BACKGROUND_IMAGE_FORMAT}。`}>
            <input ref={fileInput} type="file" accept={BACKGROUND_IMAGE_TYPES.join(',')} onChange={handleBgImageUpload} hidden />
            <Button variant="secondary" onClick={() => fileInput.current?.click()}>{bgImage ? '更换背景图片' : '选择背景图片'}</Button>
            {bgImage && <Button variant="ghost" onClick={() => { imageRead.current?.abort(); imageRead.current = null; update({ bgImage: '' }); setImageError(''); }}>移除背景</Button>}
          </SettingsRow>
          {imageError && <p role="alert" className="ui-notice ui-notice--danger">{imageError}</p>}
          {bgImage && <>
            <div className="slider-field">
              <label htmlFor="bg-opacity">背景不透明度</label>
              <div className="slider-row">
                <input id="bg-opacity" type="range" min="0" max="100" value={bgOpacity}
                  onChange={(event) => update({ bgOpacity: Number(event.target.value) })} />
                <output className="slider-value" htmlFor="bg-opacity">{bgOpacity}%</output>
              </div>
            </div>
            <img src={bgImage} alt="背景预览" className="bg-preview-img" />
          </>}
        </div>
      </SettingsSection>
      <DesktopPetSettingsSection />
    </div>
  );
}
