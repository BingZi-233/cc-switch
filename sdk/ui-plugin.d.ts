export interface PluginContext {
  apiVersion: 1;
  pluginId: string;
  pageId: string;
  language: string;
  theme: "light" | "dark";
  permissions: PluginPermission[];
}

export type PluginPermission = "providers.read" | "usage.read";
export type PluginAppType =
  | "claude"
  | "claude-desktop"
  | "codex"
  | "gemini"
  | "grokbuild"
  | "opencode"
  | "openclaw"
  | "hermes"
  | "pi"
  | "mcode"
  | "kimi"
  | "agy"
  | "omp";
export interface PluginProvider {
  id: string;
  appType: PluginAppType;
  name: string;
  category: string | null;
}
export interface PluginProviderList {
  items: PluginProvider[];
  total: number;
  offset: number;
  limit: number;
  source: "registry";
}
export interface PluginUsageQuery {
  /** Unix seconds, inclusive; maximum range 366 days. */
  startDate: number;
  endDate: number;
  appType?: PluginAppType;
  /** Exact display-name match, following the main usage screen. */
  providerName?: string;
}
export interface PluginUsageStats {
  startDate: number;
  endDate: number;
  summary: {
    totalRequests: number;
    totalTokens: number;
    inputTokens: number;
    outputTokens: number;
    cacheReadTokens: number;
    cacheCreationTokens: number;
    totalCostUsd: string;
    successRate: number;
    cacheHitRate: number;
  };
  trends: { date: string; requests: number; tokens: number; costUsd: string }[];
}

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

declare global {
  interface Window {
    /** All host calls are asynchronous. Await each request; no synchronous API is exposed. */
    CCSwitch: {
      readonly ready: Promise<PluginContext>;
      getContext(): Promise<PluginContext>;
      getStorage(): Promise<JsonValue>;
      setStorage(value: JsonValue): Promise<null>;
      providers: {
        list(options?: {
          appType?: PluginAppType;
          offset?: number;
          limit?: number;
        }): Promise<PluginProviderList>;
      };
      usage: { getStats(options: PluginUsageQuery): Promise<PluginUsageStats> };
    };
  }
  const CCSwitch: Window["CCSwitch"];
  interface WindowEventMap {
    "cc-switch-context-changed": CustomEvent<PluginContext>;
  }
}
