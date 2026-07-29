// Thin typed wrapper over the Tauri command surface. Field names match the
// Rust structs verbatim (serde uses the Rust field names as-is).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type CaptureState = "starting" | "running" | "idle" | "failed";

export interface CaptureStatus {
  state: CaptureState;
  profile_name: string;
  messages_written: number;
  bytes_written: number;
  started_at_ms: number | null;
  last_message_at_ms: number | null;
  last_error: string | null;
}

export interface Profile {
  name: string;
  endpoint: string;
  mode: string;
  namespace: string | null;
  key_expr: string;
  output_dir: string;
  rotate_size_bytes: number;
  rotate_interval_secs: number;
  max_total_size_bytes: number;
  max_age_secs: number;
}

export interface AppSettings {
  selected_profile: string;
  resume_capture_on_launch: boolean;
  last_capture_running: boolean;
}

export interface AppConfig {
  schema_version: number;
  app: AppSettings;
  profiles: Profile[];
}

export const getConfig = () => invoke<AppConfig>("get_config");
export const saveConfig = (config: AppConfig) =>
  invoke<void>("save_config", { config });
export const getStatus = () => invoke<CaptureStatus>("get_status");
export const toggleCapture = () => invoke<void>("toggle_capture");
export const selectProfile = (name: string) =>
  invoke<void>("select_profile", { name });
export const openStoreFolder = () => invoke<void>("open_store_folder");
export const newProfile = (name: string) => invoke<Profile>("new_profile", { name });
export const hideSettings = () => invoke<void>("hide_settings");

export const CAPTURE_STATUS_EVENT = "capture-status";

/** Subscribe to backend status pushes. Returns an unlisten function. */
export const onCaptureStatus = (handler: (status: CaptureStatus) => void) =>
  listen<CaptureStatus>(CAPTURE_STATUS_EVENT, (e) => handler(e.payload));
