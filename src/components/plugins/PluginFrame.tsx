import { useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import {
  uiPluginsApi,
  type InstalledPlugin,
  type PluginPage,
} from "@/lib/api/uiPlugins";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";

export const PLUGIN_CHANNEL = "cc-switch-plugin/v1";

/** No generic invoke bridge: identity comes from this mounted frame, never the payload. */
export function PluginFrame({
  plugin,
  page,
}: {
  plugin: InstalledPlugin;
  page: PluginPage;
}) {
  const { t, i18n } = useTranslation();
  const frame = useRef<HTMLIFrameElement>(null);
  const [attempt, setAttempt] = useState(0);
  const permissionsKey = (plugin.grantedPermissions ?? []).join(",");
  const [status, setStatus] = useState<"loading" | "ready" | "error">(
    "loading",
  );

  useLayoutEffect(() => {
    let active = true;
    let busy = false;
    const target = frame.current?.contentWindow;
    const pending = new Set<string>();
    setStatus("loading");
    const timeout = setTimeout(() => setStatus("error"), 10000);
    const context = () => ({
      apiVersion: 1,
      pluginId: plugin.manifest.id,
      pageId: page.id,
      permissions: plugin.grantedPermissions ?? [],
      language: i18n.resolvedLanguage ?? i18n.language,
      theme: document.documentElement.classList.contains("dark")
        ? "dark"
        : "light",
    });
    const post = (message: object) => {
      if (active)
        target?.postMessage({ channel: PLUGIN_CHANNEL, ...message }, "*");
    };
    const notifyContext = () =>
      post({ event: "contextChanged", value: context() });
    const observer = new MutationObserver(notifyContext);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    i18n.on("languageChanged", notifyContext);
    const onMessage = async (event: MessageEvent) => {
      if (!target || event.source !== target || event.origin !== "null") return;
      const request = event.data;
      if (
        !request ||
        typeof request !== "object" ||
        request.channel !== PLUGIN_CHANNEL ||
        typeof request.requestId !== "string" ||
        request.requestId.length > 80 ||
        typeof request.method !== "string" ||
        pending.has(request.requestId)
      )
        return;
      // One asynchronous storage operation at a time avoids unbounded work and preserves write order.
      if (busy) {
        post({
          requestId: request.requestId,
          error: "Plugin host is busy; await the previous request",
        });
        return;
      }
      pending.add(request.requestId);
      try {
        let value: unknown;
        switch (request.method) {
          case "ready":
          case "getContext":
            clearTimeout(timeout);
            setStatus("ready");
            value = context();
            break;
          case "getStorage":
            busy = true;
            value = await uiPluginsApi.getStorage(plugin.manifest.id);
            break;
          case "providers.list":
            busy = true;
            value = await uiPluginsApi.getProviders(
              plugin.manifest.id,
              request.value ?? {},
            );
            break;
          case "usage.getStats":
            busy = true;
            value = await uiPluginsApi.getUsage(
              plugin.manifest.id,
              request.value ?? {},
            );
            break;
          case "setStorage": {
            const json = JSON.stringify(request.value);
            if (
              json === undefined ||
              new TextEncoder().encode(json).length > 65536
            )
              throw new Error("Storage must be JSON within 64 KiB");
            busy = true;
            await uiPluginsApi.setStorage(plugin.manifest.id, JSON.parse(json));
            value = null;
            break;
          }
          default:
            throw new Error("Unsupported plugin method");
        }
        post({ requestId: request.requestId, value });
      } catch (error) {
        post({
          requestId: request.requestId,
          error: error instanceof Error ? error.message : String(error),
        });
      } finally {
        busy = false;
        pending.delete(request.requestId);
      }
    };
    window.addEventListener("message", onMessage);
    return () => {
      active = false;
      clearTimeout(timeout);
      observer.disconnect();
      i18n.off("languageChanged", notifyContext);
      window.removeEventListener("message", onMessage);
    };
  }, [plugin.manifest.id, permissionsKey, page.id, attempt, i18n]);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      {status === "loading" && (
        <div
          role="status"
          className="absolute inset-x-0 top-0 flex items-center gap-2 bg-surface px-6 py-3 text-body text-fg-2"
        >
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("plugins.loadingPage")}
        </div>
      )}
      {status === "error" && (
        <div
          role="alert"
          className="absolute inset-x-0 top-0 z-10 bg-surface p-4"
        >
          <Notice
            tone="danger"
            title={t("plugins.pageFailed")}
            actions={
              <Button
                variant="neutral"
                onClick={() => setAttempt((n) => n + 1)}
              >
                {t("common.retry")}
              </Button>
            }
          />
        </div>
      )}
      <iframe
        key={`${plugin.manifest.id}/${page.id}/${attempt}`}
        ref={frame}
        title={`${plugin.manifest.name} — ${page.title}`}
        sandbox="allow-scripts"
        referrerPolicy="no-referrer"
        allow="camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'"
        className="min-h-0 w-full flex-1 border-0 bg-surface"
        src={uiPluginsApi.pageUrl(plugin.manifest.id, page.id)}
      />
    </div>
  );
}
