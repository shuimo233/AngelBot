import { useSettingsStore } from '$stores/settings';
import { getModelConfigReadiness, openModelSettings } from '$lib/model-config';

export function ModelConfigurationNotice() {
  const config = useSettingsStore((state) => state.activeApiConfig);
  const loaded = useSettingsStore((state) => state.apiConfigLoaded);
  const loadError = useSettingsStore((state) => state.apiConfigLoadError);
  const load = useSettingsStore((state) => state.loadApiConfig);
  const readiness = getModelConfigReadiness(config, loaded, loadError);

  if (readiness.kind === 'loading' || readiness.kind === 'ready') return null;

  return (
    <aside className="model-config-notice" role={readiness.kind === 'needs-credentials' ? 'status' : 'alert'}>
      <div className="model-config-notice__copy">
        <strong>{readiness.title}</strong>
        <span>{readiness.detail}</span>
      </div>
      <div className="model-config-notice__actions">
        {(loadError || config.planStatusError) && <button type="button" onClick={() => { void load(); }}>重新检查</button>}
        <button type="button" className="is-primary" onClick={openModelSettings}>
          {readiness.kind === 'needs-credentials' ? '连接模型' : '模型设置'}
        </button>
      </div>
    </aside>
  );
}
