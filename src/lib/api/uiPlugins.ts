import { convertFileSrc, invoke } from "@tauri-apps/api/core";

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
  getProviders: (id: string, query: unknown) =>
    invoke<unknown>("get_ui_plugin_providers", { id, query }),
  getUsage: (id: string, query: unknown) =>
    invoke<unknown>("get_ui_plugin_usage", { id, query }),
  uninstall: (id: string) => invoke<void>("uninstall_ui_plugin", { id }),
  getStorage: (id: string) => invoke<unknown>("get_ui_plugin_storage", { id }),
  setStorage: (id: string, value: unknown) =>
    invoke<void>("set_ui_plugin_storage", { id, value }),
  pageUrl: (id: string, page: string) =>
    `${convertFileSrc(id, "cc-plugin")}/${encodeURIComponent(page)}`,
};
