import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ExternalLink, Loader2, Square } from "lucide-react";
import {
  uiPluginsApi,
  type InstalledPlugin,
  type PluginPage,
  type PluginRuntimeStatus,
} from "@/lib/api/uiPlugins";
import { Button } from "@/components/ui/button";
import { Notice, NoticeSlot } from "@/components/ui/notice";
import { extractErrorMessage } from "@/utils/errorUtils";

/** Third-party HTML never enters the application's renderer. */
export function PluginRuntime({
  plugin,
  page,
}: {
  plugin: InstalledPlugin;
  page: PluginPage;
}) {
  const { t, i18n } = useTranslation();
  const [attempt, setAttempt] = useState(0);
  const [status, setStatus] = useState<PluginRuntimeStatus>();
  const [error, setError] = useState<string>();
  const [pending, setPending] = useState(false);
  const stopped = useRef(false);

  useEffect(() => {
    let active = true;
    let sessionId: string | undefined;
    let timer: ReturnType<typeof setTimeout>;
    setStatus(undefined);
    setError(undefined);
    setPending(false);
    stopped.current = false;
    const appearance = () => ({
      language: i18n.resolvedLanguage ?? i18n.language,
      theme: document.documentElement.classList.contains("dark")
        ? ("dark" as const)
        : ("light" as const),
    });
    const stop = (id: string) => uiPluginsApi.stopRuntime(id).catch(() => {});
    const syncContext = () => {
      if (sessionId)
        void uiPluginsApi
          .updateRuntime(sessionId, appearance())
          .catch(() => {});
    };
    const poll = async () => {
      if (!active || !sessionId || stopped.current) return;
      try {
        const next = await uiPluginsApi.getRuntime(sessionId);
        if (!active || stopped.current) return;
        setStatus(next);
        if (next.state === "starting" || next.state === "running")
          timer = setTimeout(poll, 1000);
      } catch (error) {
        if (active && !stopped.current) {
          setError(extractErrorMessage(error));
          void stop(sessionId);
        }
      }
    };
    const observer = new MutationObserver(syncContext);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    i18n.on("languageChanged", syncContext);
    void uiPluginsApi
      .startRuntime(plugin.manifest.id, page.id, appearance())
      .then((next) => {
        sessionId = next.sessionId;
        if (!active) {
          void stop(sessionId);
          return;
        }
        setStatus(next);
        syncContext();
        void poll();
      })
      .catch((error) => {
        if (active) setError(extractErrorMessage(error));
      });
    return () => {
      active = false;
      clearTimeout(timer);
      observer.disconnect();
      i18n.off("languageChanged", syncContext);
      if (sessionId) void stop(sessionId);
    };
  }, [plugin.manifest.id, page.id, attempt, i18n]);

  const state = error ? "failed" : (status?.state ?? "starting");
  const alive = state === "starting" || state === "running";
  const control = async (focus: boolean) => {
    if (!status) return;
    setPending(true);
    try {
      if (focus) await uiPluginsApi.updateRuntime(status.sessionId);
      else {
        stopped.current = true;
        await uiPluginsApi.stopRuntime(status.sessionId);
        setStatus({ ...status, state: "closed" });
      }
    } catch (error) {
      setError(extractErrorMessage(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
      <NoticeSlot>
        <Notice
          tone={
            state === "failed" || state === "unresponsive"
              ? "danger"
              : "neutral"
          }
          title={t(`plugins.runtime_${state}`)}
        >
          {error ?? t("plugins.runtimeHint")}
        </Notice>
      </NoticeSlot>
      <div className="mt-4 flex flex-wrap items-center gap-2">
        {alive ? (
          <>
            <Button
              variant="neutral"
              disabled={!status || pending}
              onClick={() => void control(true)}
            >
              {state === "starting" ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <ExternalLink className="h-4 w-4" />
              )}
              {t("plugins.showWindow")}
            </Button>
            <Button
              variant="neutral"
              disabled={!status || pending}
              onClick={() => void control(false)}
            >
              <Square className="h-4 w-4" />
              {t("plugins.stopRuntime")}
            </Button>
          </>
        ) : (
          <Button variant="neutral" onClick={() => setAttempt((n) => n + 1)}>
            {t("plugins.reopen")}
          </Button>
        )}
      </div>
    </div>
  );
}
