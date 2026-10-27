import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowLeft,
  FolderPlus,
  Loader2,
  Puzzle,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  uiPluginsApi,
  uiPluginsKey,
  type InstalledPlugin,
  type PluginPermission,
} from "@/lib/api/uiPlugins";
import { AppPageHeader } from "@/components/shell/AppPageHeader";
import { Button } from "@/components/ui/button";
import { HoverTip } from "@/components/ui/hover-tip";
import { Notice, NoticeSlot } from "@/components/ui/notice";
import { PageTabs } from "@/components/ui/page-tabs";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { toast } from "@/lib/toast";
import { extractErrorMessage } from "@/utils/errorUtils";
import { PluginRuntime } from "./PluginRuntime";
import { useLastValidValue } from "@/hooks/useLastValidValue";
import { PluginPermissionsDialog } from "./PluginPermissionsDialog";

type Action =
  | { kind: "install" }
  | {
      kind: "enable" | "disable" | "uninstall" | "permissions";
      id: string;
      name: string;
      grants?: PluginPermission[];
    };

export function PluginsPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: uiPluginsKey,
    queryFn: uiPluginsApi.list,
  });
  const [selection, setSelection] = useState<{
    plugin: string;
    page: string;
  }>();
  const [confirmation, setConfirmation] =
    useState<Exclude<Action, { kind: "install" }>>();
  const dialogAction = useLastValidValue(confirmation);
  const [openPages, setOpenPages] = useState<
    { plugin: string; page: string; revision: number }[]
  >([]);
  const [permissionTarget, setPermissionTarget] = useState<{
    plugin: InstalledPlugin;
    enabling: boolean;
  }>();
  const mutation = useMutation({
    mutationFn: async (action: Action) => {
      if (action.kind === "install") {
        const directory = await open({
          directory: true,
          multiple: false,
          title: t("plugins.chooseFolder"),
        });
        if (typeof directory !== "string") return false;
        const installed = await uiPluginsApi.install(directory);
        return installed.manifest.id;
      } else if (action.kind === "uninstall") {
        await uiPluginsApi.uninstall(action.id);
      } else if (action.kind === "permissions") {
        await uiPluginsApi.setPermissions(action.id, action.grants ?? []);
      } else {
        await uiPluginsApi.setEnabled(
          action.id,
          action.kind === "enable",
          action.grants,
        );
      }
      return true;
    },
    onSuccess: async (changed, action) => {
      setConfirmation(undefined);
      setPermissionTarget(undefined);
      if (!changed) return;
      if (action.kind === "install" && typeof changed === "string") {
        setOpenPages((previous) =>
          previous.filter((item) => item.plugin !== changed),
        );
      } else if (action.kind === "disable" || action.kind === "uninstall") {
        setOpenPages((previous) =>
          previous.filter((item) => item.plugin !== action.id),
        );
      }
      await queryClient.invalidateQueries({ queryKey: uiPluginsKey });
      toast.success(t(`plugins.${action.kind}Done`));
    },
    onError: (_error, action) => {
      if (action.kind === "uninstall")
        void queryClient.invalidateQueries({ queryKey: uiPluginsKey });
    },
  });
  const plugin = query.data?.plugins.find(
    (item) => item.manifest.id === selection?.plugin && item.enabled,
  );
  const page = plugin?.manifest.pages.find(
    (item) => item.id === selection?.page,
  );
  const openPage = (plugin: string, page: string) => {
    setOpenPages((previous) =>
      previous.some((item) => item.plugin === plugin && item.page === page)
        ? previous
        : [...previous, { plugin, page, revision: 0 }],
    );
    setSelection({ plugin, page });
  };
  const openPlugin = (item: InstalledPlugin) =>
    openPage(item.manifest.id, item.manifest.pages[0].id);

  const permissionButton = (item: InstalledPlugin) =>
    item.enabled &&
    Boolean(item.manifest.permissions?.length) && (
      <HoverTip content={t("plugins.permissions")}>
        <Button
          variant="ghost"
          size="icon"
          aria-label={t("plugins.permissionsNamed", {
            name: item.manifest.name,
          })}
          disabled={mutation.isPending}
          onClick={() => {
            mutation.reset();
            setPermissionTarget({ plugin: item, enabling: false });
          }}
        >
          <ShieldCheck className="h-4 w-4" />
        </Button>
      </HoverTip>
    );
  const permissionDialog = permissionTarget && (
    <PluginPermissionsDialog
      key={permissionTarget.plugin.manifest.id}
      plugin={permissionTarget.plugin}
      enabling={permissionTarget.enabling}
      pending={mutation.isPending}
      error={mutation.error ? extractErrorMessage(mutation.error) : undefined}
      onClose={() => setPermissionTarget(undefined)}
      onSave={(grants) =>
        mutation.mutate({
          kind: permissionTarget.enabling ? "enable" : "permissions",
          id: permissionTarget.plugin.manifest.id,
          name: permissionTarget.plugin.manifest.name,
          grants,
        })
      }
    />
  );

  const uninstallButton = (id: string, name: string) => (
    <HoverTip content={t("plugins.uninstall")}>
      <Button
        variant="ghost"
        size="icon"
        disabled={mutation.isPending}
        aria-label={t("plugins.uninstallNamed", { name })}
        onClick={() => {
          mutation.reset();
          setConfirmation({ kind: "uninstall", id, name });
        }}
      >
        <Trash2 className="h-4 w-4" />
      </Button>
    </HoverTip>
  );

  const detail =
    plugin && page ? (
      <>
        <AppPageHeader
          title={plugin.manifest.name}
          truncateTitle
          leading={
            <HoverTip content={t("plugins.back")}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("plugins.back")}
                onClick={() => setSelection(undefined)}
              >
                <ArrowLeft className="h-4 w-4" />
              </Button>
            </HoverTip>
          }
          actions={
            <>
              {permissionButton(plugin)}
              <Button
                variant="neutral"
                onClick={() =>
                  setOpenPages((previous) =>
                    previous.map((item) =>
                      item.plugin === plugin.manifest.id &&
                      item.page === page.id
                        ? { ...item, revision: item.revision + 1 }
                        : item,
                    ),
                  )
                }
              >
                <RefreshCw className="h-4 w-4" />
                {t("plugins.reload")}
              </Button>
            </>
          }
        />
        <div className="shrink-0 overflow-x-auto px-6">
          <PageTabs
            value={page.id}
            onValueChange={(id) => openPage(plugin.manifest.id, id)}
            items={plugin.manifest.pages.map((item) => ({
              value: item.id,
              label: item.title,
            }))}
            aria-label={t("plugins.pages")}
            idPrefix="plugin-tab"
            controls="plugin-content"
          />
        </div>
        {permissionDialog}
      </>
    ) : null;

  return (
    <>
      {detail ?? (
        <>
          <AppPageHeader
            title={t("plugins.title")}
            truncateTitle
            icon={<Puzzle className="h-5 w-5" strokeWidth={1.5} />}
            actions={
              <Button
                disabled={mutation.isPending}
                aria-label={t("plugins.install")}
                onClick={() => mutation.mutate({ kind: "install" })}
              >
                <FolderPlus className="h-4 w-4" />
                <span className="hidden sm:inline">{t("plugins.install")}</span>
                <span className="sm:hidden">{t("plugins.installShort")}</span>
              </Button>
            }
          />
          <div
            id="main-content"
            className="min-h-0 flex-1 overflow-y-auto scroll-stable px-6 pb-10 pt-4"
          >
            <p className="mb-4 text-body text-fg-2">
              {t("plugins.description")}
            </p>
            <NoticeSlot className="mb-4">
              {(query.error || mutation.error) && (
                <Notice
                  tone="danger"
                  title={t("plugins.operationFailed")}
                  actions={
                    query.error ? (
                      <Button
                        variant="neutral"
                        onClick={() => void query.refetch()}
                      >
                        {t("common.retry")}
                      </Button>
                    ) : undefined
                  }
                >
                  <span className="break-words">
                    {extractErrorMessage(mutation.error ?? query.error)}
                  </span>
                </Notice>
              )}
            </NoticeSlot>
            {query.isPending && (
              <div
                role="status"
                className="flex min-h-24 items-center justify-center gap-2 text-body text-fg-2"
              >
                <Loader2 className="h-4 w-4 animate-spin" />
                {t("common.loading")}
              </div>
            )}
            {query.data &&
              query.data.plugins.length === 0 &&
              query.data.problems.length === 0 && (
                <div className="rounded-panel bg-subtle px-5 py-8">
                  <h2 className="text-strong font-semibold">
                    {t("plugins.empty")}
                  </h2>
                  <p className="mt-2 text-body text-fg-2">
                    {t("plugins.emptyHint")}
                  </p>
                </div>
              )}
            <ul className="divide-y divide-border">
              {query.data?.plugins.map((item) => (
                <li
                  key={item.manifest.id}
                  className="flex flex-wrap items-center gap-3 py-4"
                >
                  <div className="min-w-0 flex-1 basis-48">
                    <h2 className="break-words text-strong font-semibold">
                      {item.manifest.name}
                    </h2>
                    <p className="mt-1 break-all text-caption text-fg-2">
                      {item.manifest.id} · {item.manifest.version} ·{" "}
                      {t(item.enabled ? "plugins.enabled" : "plugins.disabled")}
                    </p>
                    {item.manifest.description && (
                      <p className="mt-1 break-words text-body text-fg-2">
                        {item.manifest.description}
                      </p>
                    )}
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <Button
                      variant="neutral"
                      disabled={!item.enabled || mutation.isPending}
                      onClick={() => openPlugin(item)}
                    >
                      {t("plugins.open")}
                    </Button>
                    <Button
                      variant="quiet"
                      disabled={mutation.isPending}
                      onClick={() => {
                        mutation.reset();
                        const action = {
                          kind: item.enabled
                            ? ("disable" as const)
                            : ("enable" as const),
                          id: item.manifest.id,
                          name: item.manifest.name,
                        };
                        if (item.enabled) mutation.mutate(action);
                        else if (item.manifest.permissions?.length)
                          setPermissionTarget({ plugin: item, enabling: true });
                        else setConfirmation(action);
                      }}
                    >
                      {t(item.enabled ? "plugins.disable" : "plugins.enable")}
                    </Button>
                    {permissionButton(item)}
                    {uninstallButton(item.manifest.id, item.manifest.name)}
                  </div>
                </li>
              ))}
              {query.data?.problems.map((problem) => (
                <li key={problem.id} className="flex items-start gap-3 py-4">
                  <div className="min-w-0 flex-1">
                    <h2 className="break-all text-strong font-semibold">
                      {problem.id}
                    </h2>
                    <p className="mt-1 text-body text-danger-text">
                      {t("plugins.invalid")}
                    </p>
                    <p className="mt-1 break-words text-caption text-fg-2">
                      {problem.error}
                    </p>
                  </div>
                  {uninstallButton(problem.id, problem.id)}
                </li>
              ))}
            </ul>
          </div>
          <ConfirmDialog
            isOpen={Boolean(confirmation)}
            pending={mutation.isPending}
            variant={dialogAction?.kind === "enable" ? "info" : "destructive"}
            title={t(
              dialogAction?.kind === "enable"
                ? "plugins.enableTitle"
                : "plugins.uninstallTitle",
              { name: dialogAction?.name },
            )}
            message={
              t(
                dialogAction?.kind === "enable"
                  ? "plugins.enableMessage"
                  : "plugins.uninstallMessage",
              ) +
              (mutation.error
                ? `\n\n${extractErrorMessage(mutation.error)}`
                : "")
            }
            confirmText={t(
              dialogAction?.kind === "enable"
                ? "plugins.enable"
                : "plugins.uninstall",
            )}
            onCancel={() => setConfirmation(undefined)}
            onConfirm={() => {
              if (confirmation) mutation.mutate(confirmation);
            }}
          />
          {permissionDialog}
        </>
      )}
      {openPages.map((item) => {
        const currentPlugin = query.data?.plugins.find(
          (candidate) =>
            candidate.manifest.id === item.plugin && candidate.enabled,
        );
        const currentPage = currentPlugin?.manifest.pages.find(
          (candidate) => candidate.id === item.page,
        );
        if (!currentPlugin || !currentPage) return null;
        const visible =
          plugin?.manifest.id === item.plugin && page?.id === item.page;
        return (
          <div
            key={`${item.plugin}/${item.page}`}
            id={visible ? "main-content" : undefined}
            hidden={!visible}
            className={visible ? "flex min-h-0 flex-1 flex-col" : undefined}
          >
            <div
              id={visible ? "plugin-content" : undefined}
              role="tabpanel"
              aria-labelledby={visible ? `plugin-tab-${item.page}` : undefined}
              className="flex min-h-0 flex-1 flex-col"
            >
              <PluginRuntime
                key={`${item.revision}/${currentPlugin.grantedPermissions?.join(",") ?? ""}`}
                plugin={currentPlugin}
                page={currentPage}
              />
            </div>
          </div>
        );
      })}
    </>
  );
}
