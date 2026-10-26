import { useState, useEffect, useRef, useCallback } from 'react';
import { SettingsRouter } from './SettingsRouter';
import type { SettingsPage } from './types';
import { hasUnsavedApiConfig, useSettingsStore } from '$stores/settings';
import { useDialogFocus } from '$lib/useDialogFocus';

interface SettingsModalProps {
  isOpen: boolean;
  initialPage?: SettingsPage;
  onClose: () => void;
}

export function SettingsModal({ isOpen, initialPage = 'personality', onClose }: SettingsModalProps) {
  const [activePage, setActivePage] = useState<SettingsPage>('personality');
  const [saveState, setSaveState] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [closeRequested, setCloseRequested] = useState(false);
  const loadProfile = useSettingsStore((s) => s.loadProfile);
  const loadApiConfig = useSettingsStore((s) => s.loadApiConfig);
  const persistProfile = useSettingsStore((s) => s.persistProfile);
  const persistApiConfig = useSettingsStore((s) => s.persistApiConfig);
  const apiConfig = useSettingsStore((s) => s.apiConfig);
  const activeApiConfig = useSettingsStore((s) => s.activeApiConfig);
  const discardApiConfigDraft = useSettingsStore((s) => s.discardApiConfigDraft);
  const profile = useSettingsStore((s) => s.profile);
  const apiConfigLoadError = useSettingsStore((s) => s.apiConfigLoadError);
  const savedApiConfig = useRef(apiConfig);
  const savedProfile = useRef(profile);
  const activePageRef = useRef(activePage);
  activePageRef.current = activePage;
  const openGeneration = useRef(0);
  const unsaved = hasUnsavedApiConfig(apiConfig, activeApiConfig)
    || JSON.stringify(profile) !== JSON.stringify(savedProfile.current);
  const requiresExplicitSave = activePage === 'personality' || activePage === 'api';
  const requestClose = useCallback(() => {
    if (saveState === 'saving' || saveState === 'error' || unsaved) setCloseRequested(true);
    else onClose();
  }, [saveState, unsaved, onClose]);
  const dialogRef = useDialogFocus<HTMLDivElement>({
    isOpen,
    onClose: requestClose,
    initialFocusSelector: '.settings-close-btn',
  });

  useEffect(() => {
    if (isOpen) {
      ++openGeneration.current;
      setCloseRequested(false);
      setSaveState('idle');
      loadProfile();
      savedProfile.current = useSettingsStore.getState().profile;
      loadApiConfig();
    }
    return () => { ++openGeneration.current; };
  }, [isOpen, loadProfile, loadApiConfig]);

  useEffect(() => { if (isOpen) setActivePage(initialPage); }, [initialPage, isOpen]);

  useEffect(() => {
    if (isOpen && closeRequested && !unsaved && saveState !== 'saving' && saveState !== 'error') {
      setCloseRequested(false);
      onClose();
    }
  }, [isOpen, closeRequested, unsaved, saveState, onClose]);

  useEffect(() => {
    setSaveState((state) => state === 'saving' ? state : 'idle');
  }, [activePage]);

  useEffect(() => {
    const draftChanged = activePage === 'api'
      ? apiConfig !== savedApiConfig.current : profile !== savedProfile.current;
    setSaveState((state) => state === 'saved' && draftChanged ? 'idle' : state);
  }, [apiConfig, profile, activePage]);

  useEffect(() => {
    if (saveState !== 'saved') return;
    const timer = window.setTimeout(() => setSaveState('idle'), 2000);
    return () => window.clearTimeout(timer);
  }, [saveState]);

  const handleSave = async () => {
    const generation = openGeneration.current;
    setSaveState('saving');
    try {
      if (activePage === 'personality') persistProfile();
      if (activePage === 'api') await persistApiConfig();
      if (openGeneration.current !== generation) return;
      const state = useSettingsStore.getState();
      if (activePage === 'api') savedApiConfig.current = state.apiConfig;
      if (activePage === 'personality') savedProfile.current = state.profile;
      setSaveState(activePageRef.current !== activePage || (activePage === 'api' && state.apiConfig !== state.activeApiConfig) ? 'idle' : 'saved');
    } catch {
      if (openGeneration.current === generation) setSaveState('error');
    }
  };

  if (!isOpen) return null;

  return (
    <div className="settings-overlay" onClick={requestClose}>
      <div
        ref={dialogRef}
        className="settings-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-dialog-title"
        aria-describedby="settings-save-hint"
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="settings-header">
          <button className="settings-close-btn" onClick={requestClose} aria-label="关闭">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
          <h2 id="settings-dialog-title" className="settings-header-title">设置</h2>
          <span id="settings-save-hint" className={`settings-save-hint${saveState === 'error' ? ' is-error' : ''}`} role={saveState === 'error' ? 'alert' : undefined}>
            {saveState === 'error'
              ? activePage === 'api' && apiConfigLoadError
                ? `无法确认模型配置已生效。${apiConfigLoadError}`
                : activePage === 'api' ? '模型配置未保存，请检查必填项后重试' : '性格修改未保存，请重试'
              : activePage === 'api' ? '模型与凭据修改需保存' : activePage === 'personality' ? '性格修改需保存' : activePage === 'webSearch' ? '联网搜索配置需在页面内保存' : '此页设置即时生效'}
          </span>
          {requiresExplicitSave && (
            <button
              className={`settings-save-btn ${saveState === 'saved' ? 'success' : ''}`}
              onClick={() => { void handleSave(); }}
              disabled={saveState === 'saving'}
            >
              {saveState === 'saved' ? (
                <>
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                  已保存
                </>
              ) : saveState === 'saving' ? '保存中…' : saveState === 'error' ? '重试保存' : '保存'}
            </button>
          )}
        </div>
        {closeRequested && <div className="settings-leave-prompt" role="alert">
          <p>{saveState === 'saving'
            ? '正在保存，完成后将核对是否可以关闭。关闭设置不会取消已提交的保存。'
            : saveState === 'error'
              ? '本次保存未能确认成功。请先核对配置；关闭不会自动重试保存。'
              : '还有未保存的模型或性格修改，离开前请保存，或明确放弃修改。'}</p>
          <div>
            <button type="button" onClick={() => setCloseRequested(false)}>继续编辑</button>
            <button type="button" disabled={saveState === 'saving'} onClick={() => {
              setCloseRequested(false);
              loadProfile();
              discardApiConfigDraft();
              onClose();
            }}>放弃未保存修改并关闭</button>
          </div>
        </div>}
        <SettingsRouter activePage={activePage} onNavigate={setActivePage} />
      </div>
    </div>
  );
}
