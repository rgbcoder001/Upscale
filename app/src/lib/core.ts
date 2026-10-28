import { invoke, isTauri } from "@tauri-apps/api/core";

export interface AppInfo {
  coreVersion: string;
}

/** Version info from the Rust core, or null when running outside Tauri (tests, plain browser). */
export async function getAppInfo(): Promise<AppInfo | null> {
  if (!isTauri()) return null;
  return invoke<AppInfo>("app_info");
}
