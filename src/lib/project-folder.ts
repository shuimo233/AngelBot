import { open } from '@tauri-apps/plugin-dialog';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';

/** Reuse the existing project admission: a folder never grants Daily file access. */
export async function openProjectFolder(): Promise<boolean> {
  const navigation = useNavigationStore.getState();
  const selected = await open({ directory: true, multiple: false, title: '选择项目文件夹' });
  if (typeof selected !== 'string') return false;
  const accepted = await useWorkspacesStore.getState().createProject(selected);
  if (!accepted || navigation !== useNavigationStore.getState()) return false;
  navigation.setPage('chat');
  return true;
}
