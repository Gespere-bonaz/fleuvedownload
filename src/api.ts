import { invoke } from "@tauri-apps/api/core";
import type { AddRequest, DownloadView, PairingInfo, QualityOption, Settings } from "./types";

export const api = {
  list: () => invoke<DownloadView[]>("list_downloads"),
  add: (requests: AddRequest[]) => invoke<string[]>("add_downloads", { requests }),
  pause: (id: string) => invoke<void>("pause_download", { id }),
  pauseAll: () => invoke<number>("pause_all"),
  clearList: (all: boolean) => invoke<number>("clear_list", { all }),
  resume: (id: string) => invoke<void>("resume_download", { id }),
  schedule: (id: string, at: number) => invoke<void>("schedule_download", { id, at }),
  remove: (id: string, deleteFile: boolean) => invoke<void>("remove_download", { id, deleteFile }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  open: (id: string) => invoke<void>("open_download", { id }),
  reveal: (id: string) => invoke<void>("reveal_download", { id }),
  setQuality: (id: string, quality: string) => invoke<void>("set_quality", { id, quality }),
  isVideoUrl: (url: string) => invoke<boolean>("is_video_url", { url }),
  probeQualities: (url: string, referer?: string | null) => invoke<QualityOption[]>("probe_qualities", { url, referer: referer || null }),
  pairingPending: () => invoke<PairingInfo | null>("pairing_pending"),
  pairingDecide: (id: string, approve: boolean) => invoke<void>("pairing_decide", { id, approve }),
  probeQualitiesFor: (id: string) => invoke<QualityOption[]>("probe_qualities_for", { id }),
};
