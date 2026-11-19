import './ui/tokens.css';
import './styles/global.css';
import App from './App';
import { usePreferencesStore } from './stores/preferences';
import './styles/product.css';
import './ui/system.css';

void usePreferencesStore.getState().loadPreferences();

export function MainWindow() {
  return <App />;
}
