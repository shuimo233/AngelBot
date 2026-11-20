import { useState, useEffect, useRef, useCallback } from 'react';
import { SettingsRouter } from './SettingsRouter';
import type { SettingsPage } from './types';
import { hasUnsavedApiConfig, useSettingsStore } from '$stores/settings';
import { useDialogFocus } from '$lib/useDialogFocus';
import { InlineNotice } from '../../ui';

interface SettingsModalProps {
  isOpen: boolean;
  initialPage?: SettingsPage;
  onClose: () => void;
}

export function SettingsModal({ isOpen, initialPage = 'personality', onClose }: SettingsModalProps) {
  const [activePage, setActivePage] = useState<SettingsPage>(initialPage);
  const [saveState, setSaveState] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [saveError, setSaveError] = useState('');
  const [closeRequested, setCloseRequested] = useState(false);
  const loadProfile = useSettingsStore((s) => s.loadProfile);
  const loadApiConfig = useSettingsStore((s) => s.loadApiConfig);
  const persistProfile = useSettingsStore((s) => s.persistProfile);
  const persistApiConfig = useSettingsStore((s) => s.persistApiConfig);
  const apiConfig = useSettingsStore((s) => s.apiConfig);
  const activeApiConfig = useSettingsStore((s) => s.activeApiConfig);
  const discardApiConfigDraft = useSettingsStore((s) => s.discardApiConfigDraft);
  const profile = useSettingsStore((s) => s.profile);
  const activeProfile = useSettingsStore((s) => s.activeProfile);
  const apiConfigLoadError = useSettingsStore((s) => s.apiConfigLoadError);
  const savedApiConfig = useRef(apiConfig);
  const savedProfile = useRef(profile);
  const activePageRef = useRef(activePage);
  activePageRef.current = activePage;
  const openGeneration = useRef(0);
  const unsaved = hasUnsavedApiConfig(apiConfig, activeApiConfig)
    || JSON.stringify(profile) !== JSON.stringify(activeProfile);
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
      setSaveError('');
      const { profile: draft, activeProfile } = useSettingsStore.getState();
      if (draft === activeProfile) loadProfile();
      savedProfile.current = useSettingsStore.getState().activeProfile;
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
    setSaveError('');
    try {
      if (activePage === 'personality') persistProfile();
      if (activePage === 'api') await persistApiConfig();
      if (openGeneration.current !== generation) return;
      const state = useSettingsStore.getState();
      if (activePage === 'api') savedApiConfig.current = state.apiConfig;
      if (activePage === 'personality') savedProfile.current = state.profile;
      setSaveState(activePageRef.current !== activePage || (activePage === 'api' && state.apiConfig !== state.activeApiConfig) ? 'idle' : 'saved');
    } catch (error) {
      if (openGeneration.current === generation) {
        if (activePageRef.current !== activePage) {
          setSaveState('idle');
          return;
        }
        setSaveError(error instanceof Error ? error.message : String(error));
        setSaveState('error');
      }
    }
  };

  if (!isOpen) return null;

  const saveHint = saveState === 'error'
    ? activePage === 'api' && apiConfigLoadError
      ? `无法确认模型配置已生效。${apiConfigLoadError}`
      : `${activePage === 'api' ? '模型配置' : '性格修改'}保存未能确认成功${saveError ? `：${saveError}` : '，请检查后重试'}`
    : '';

  const saveAction = requiresExplicitSave && (
    <button
      className={`settings-save-btn ${saveState === 'saved' ? 'success' : ''}`}
      onClick={() => { void handleSave(); }}
      disabled={saveState === 'saving'}
    >
      {saveState === 'saved' ? '已保存' : saveState === 'saving' ? '保存中…' : saveState === 'error' ? '重试保存' : '保存'}
    </button>
  );

  return (
    <div className="settings-overlay" onClick={requestClose}>
      <div
        ref={dialogRef}
        className="settings-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-dialog-title"
        aria-describedby={saveHint ? 'settings-save-hint' : undefined}
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
          {saveHint && <InlineNotice id="settings-save-hint" className="settings-save-error" tone="danger">{saveHint}</InlineNotice>}
          {activePage !== 'api' && saveAction}
        </div>
        {closeRequested && <div className="settings-leave-prompt" role="alert">
          <p>{saveState === 'saving'
            ? '正在保存，完成后将核对是否可以关闭。关闭设置不会取消已提交的保存。'
            : saveState === 'error'
              ? '本次保存未能确认成功。请先核对配置；关闭不会自动重试保存。'
              : '还有未保存的模型或性格修改。'}</p>
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
        <SettingsRouter activePage={activePage} onNavigate={setActivePage} apiSaveAction={activePage === 'api' ? saveAction : null} />
      </div>
    </div>
  );
}
