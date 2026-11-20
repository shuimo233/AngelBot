import { invoke } from '../invoke';

export interface DesktopPetStatus {
  supported: boolean;
  enabled: boolean;
}

export const DESKTOP_PET_CHANGED = 'desktop-pet-changed';
export const DESKTOP_PET_OPEN_CHAT = 'desktop-pet-open-chat';

export function getDesktopPet(): Promise<DesktopPetStatus> {
  return invoke('get_desktop_pet');
}

export function setDesktopPet(enabled: boolean): Promise<DesktopPetStatus> {
  return invoke('set_desktop_pet', { enabled });
}

export function startDesktopPetDrag(): Promise<void> {
  return invoke('desktop_pet_start_drag');
}

export function openMainFromDesktopPet(): Promise<void> {
  return invoke('desktop_pet_open_main');
}

export function hideDesktopPet(): Promise<void> {
  return invoke('desktop_pet_hide');
}

export function resetDesktopPetPosition(): Promise<void> {
  return invoke('desktop_pet_reset_position');
}
