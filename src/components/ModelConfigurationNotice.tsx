import { useSettingsStore } from '$stores/settings';
import { getModelConfigReadiness } from '$lib/model-config';

export function ModelConfigurationNotice() {
  const config = useSettingsStore((state) => state.activeApiConfig);
  const loaded = useSettingsStore((state) => state.apiConfigLoaded);
  const loadError = useSettingsStore((state) => state.apiConfigLoadError);
  const load = useSettingsStore((state) => state.loadApiConfig);
  const readiness = getModelConfigReadiness(config, loaded, loadError);

  if (readiness.kind === 'loading' || readiness.kind === 'ready') return null;

  const openSettings = () => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'api' }));
  return (
    <aside className="model-config-notice" role={readiness.kind === 'invalid' ? 'alert' : 'status'}>
      <span className="model-config-notice__marker" aria-hidden="true" />
      <div className="model-config-notice__copy">
        <strong>{readiness.title}</strong>
        <span>{readiness.detail}</span>
      </div>
      <div className="model-config-notice__actions">
        {loadError && <button type="button" onClick={() => { void load(); }}>重新检查</button>}
        <button type="button" className="is-primary" onClick={openSettings}>模型设置</button>
      </div>
    </aside>
  );
}
