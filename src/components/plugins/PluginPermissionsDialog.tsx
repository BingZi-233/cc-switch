import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Checkbox } from "@/components/ui/checkbox";
import { Button } from "@/components/ui/button";
import type { InstalledPlugin, PluginPermission } from "@/lib/api/uiPlugins";

export function PluginPermissionsDialog({
  plugin,
  enabling,
  pending,
  error,
  onSave,
  onClose,
}: {
  plugin: InstalledPlugin;
  enabling: boolean;
  pending: boolean;
  error?: string;
  onSave: (grants: PluginPermission[]) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [grants, setGrants] = useState<PluginPermission[]>(
    enabling
      ? (plugin.manifest.permissions ?? [])
      : (plugin.grantedPermissions ?? []),
  );
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <DialogContent className="max-w-md" zIndex="alert">
        <DialogHeader>
          <DialogTitle>
            {t(enabling ? "plugins.enableTitle" : "plugins.permissionsTitle", {
              name: plugin.manifest.name,
            })}
          </DialogTitle>
          <DialogDescription>
            {t("plugins.permissionsDescription")}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 px-6 py-3">
          {(plugin.manifest.permissions ?? []).map((permission) => {
            const label = (
              {
                "providers.read": "providersRead",
                "usage.read": "usageRead",
              } satisfies Record<PluginPermission, string>
            )[permission];
            return (
              <label
                key={permission}
                className="flex cursor-pointer items-start gap-3"
              >
                <Checkbox
                  className="mt-1"
                  checked={grants.includes(permission)}
                  disabled={pending}
                  onCheckedChange={(checked) =>
                    setGrants((previous) =>
                      checked === true
                        ? [...previous, permission]
                        : previous.filter((item) => item !== permission),
                    )
                  }
                />
                <span>
                  <span className="block text-body font-semibold">
                    {t(`plugins.${label}`)}
                  </span>
                  <span className="mt-1 block text-caption text-fg-2">
                    {t(`plugins.${label}Description`)}
                  </span>
                </span>
              </label>
            );
          })}
          <p className="text-caption text-fg-2">
            {t("plugins.permissionsHint")}
          </p>
          {error && (
            <p role="alert" className="break-words text-body text-danger-text">
              {error}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="neutral" disabled={pending} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button disabled={pending} onClick={() => onSave(grants)}>
            {t(enabling ? "plugins.enable" : "plugins.savePermissions")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
