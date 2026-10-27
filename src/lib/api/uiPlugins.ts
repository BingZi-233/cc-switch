import { invoke } from "@tauri-apps/api/core";

export type PluginPermission = "providers.read" | "usage.read";

export interface PluginPage {
  id: string;
  title: string;
  entry: string;
}

export interface PluginManifest {
  apiVersion: 1;
  id: string;
  name: string;
  version: string;
  description: string;
  pages: PluginPage[];
  permissions?: PluginPermission[];
}

export interface InstalledPlugin {
  manifest: PluginManifest;
  enabled: boolean;
  grantedPermissions?: PluginPermission[];
}

export interface PluginList {
  plugins: InstalledPlugin[];
  problems: { id: string; error: string }[];
}

export const uiPluginsKey = ["ui-plugins"] as const;
export interface PluginAppearance {
  language: string;
  theme: "light" | "dark";
}
export interface PluginRuntimeStatus {
  sessionId: string;
  state: "starting" | "running" | "closed" | "unresponsive" | "failed";
  pid: number | null;
}
export const uiPluginsApi = {
  list: () => invoke<PluginList>("list_ui_plugins"),
  install: (directory: string) =>
    invoke<InstalledPlugin>("install_ui_plugin", { directory }),
  setEnabled: (
    id: string,
    enabled: boolean,
    grantedPermissions?: PluginPermission[],
  ) =>
    invoke<void>("set_ui_plugin_enabled", { id, enabled, grantedPermissions }),
  setPermissions: (id: string, grantedPermissions: PluginPermission[]) =>
    invoke<void>("set_ui_plugin_permissions", { id, grantedPermissions }),
  uninstall: (id: string) => invoke<void>("uninstall_ui_plugin", { id }),
  startRuntime: (id: string, page: string, appearance: PluginAppearance) =>
    invoke<PluginRuntimeStatus>("start_ui_plugin_runtime", {
      id,
      page,
      appearance,
    }),
  getRuntime: (sessionId: string) =>
    invoke<PluginRuntimeStatus>("get_ui_plugin_runtime", { sessionId }),
  stopRuntime: (sessionId: string) =>
    invoke<void>("stop_ui_plugin_runtime", { sessionId }),
  updateRuntime: (sessionId: string, appearance?: PluginAppearance) =>
    invoke<void>("update_ui_plugin_runtime", { sessionId, appearance }),
};
