import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PluginsPage } from "@/components/plugins/PluginsPage";
import { PluginRuntime } from "@/components/plugins/PluginRuntime";
import type { InstalledPlugin, PluginList } from "@/lib/api/uiPlugins";
import { parseView } from "@/lib/navigation";

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  install: vi.fn(),
  setEnabled: vi.fn(),
  setPermissions: vi.fn(),
  startRuntime: vi.fn(),
  getRuntime: vi.fn(),
  stopRuntime: vi.fn(),
  updateRuntime: vi.fn(),
  uninstall: vi.fn(),
  getStorage: vi.fn(),
  setStorage: vi.fn(),
  open: vi.fn(),
}));
vi.mock("@/lib/api/uiPlugins", () => ({
  uiPluginsKey: ["ui-plugins"],
  uiPluginsApi: {
    ...mocks,
  },
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));

const plugin: InstalledPlugin = {
  enabled: false,
  manifest: {
    id: "test.notes",
    name: "Test notes",
    apiVersion: 1,
    version: "1.0.0",
    description: "Local notes",
    pages: [
      { id: "notes", title: "Notes", entry: "index.html" },
      { id: "about", title: "About", entry: "about.html" },
    ],
  },
};
let state: PluginList;
function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <PluginsPage />
    </QueryClientProvider>,
  );
}
beforeEach(() => {
  Object.values(mocks).forEach((mock) => mock.mockReset());
  state = { plugins: [], problems: [] };
  mocks.list.mockImplementation(async () => structuredClone(state));
  mocks.open.mockResolvedValue("/plugin");
  mocks.install.mockImplementation(async () => {
    state.plugins.push(structuredClone(plugin));
    return plugin;
  });
  mocks.setEnabled.mockImplementation(async (id, enabled, grants) => {
    const item = state.plugins.find((p) => p.manifest.id === id)!;
    item.enabled = enabled;
    item.grantedPermissions = enabled ? (grants ?? []) : [];
  });
  mocks.setPermissions.mockImplementation(async (id, grants) => {
    state.plugins.find((p) => p.manifest.id === id)!.grantedPermissions =
      grants;
  });
  mocks.uninstall.mockImplementation(async (id) => {
    state.plugins = state.plugins.filter((p) => p.manifest.id !== id);
  });
  mocks.getStorage.mockResolvedValue({ note: "saved" });
  mocks.setStorage.mockResolvedValue(undefined);
  mocks.startRuntime.mockImplementation(async () => ({
    sessionId: `session-${mocks.startRuntime.mock.calls.length}`,
    state: "starting",
    pid: 123,
  }));
  mocks.getRuntime.mockImplementation(async (sessionId) => ({
    sessionId,
    state: "running",
    pid: 123,
  }));
  mocks.stopRuntime.mockResolvedValue(undefined);
  mocks.updateRuntime.mockResolvedValue(undefined);
});

describe("plugin management", () => {
  it("requires explicit approval and allows individual permissions to be revoked", async () => {
    state.plugins = [
      {
        ...structuredClone(plugin),
        manifest: {
          ...plugin.manifest,
          permissions: ["providers.read", "usage.read"],
        },
      },
    ];
    mount();
    await screen.findByText("Test notes");
    fireEvent.click(screen.getByText("plugins.enable"));
    expect(mocks.setEnabled).not.toHaveBeenCalled();
    expect(
      screen.getByRole("checkbox", { name: /plugins.providersRead / }),
    ).not.toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: /plugins.usageRead / }),
    ).not.toBeChecked();
    fireEvent.click(
      screen.getByRole("checkbox", { name: /plugins.providersRead / }),
    );
    fireEvent.click(
      screen.getAllByRole("button", { name: "plugins.enable" }).at(-1)!,
    );
    await waitFor(() =>
      expect(mocks.setEnabled).toHaveBeenCalledWith("test.notes", true, [
        "providers.read",
      ]),
    );
    await waitFor(() => expect(screen.getByText("plugins.open")).toBeEnabled());
    fireEvent.click(screen.getByText("plugins.open"));
    await waitFor(() => expect(mocks.startRuntime).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByLabelText("plugins.permissionsNamed"));
    fireEvent.click(
      screen.getByRole("checkbox", { name: /plugins.providersRead / }),
    );
    fireEvent.click(screen.getByText("plugins.savePermissions"));
    await waitFor(() =>
      expect(mocks.setPermissions).toHaveBeenCalledWith("test.notes", []),
    );
    await waitFor(() => expect(mocks.startRuntime).toHaveBeenCalledTimes(2));
  });
  it("installs disabled, confirms activation, opens both pages, disables and uninstalls", async () => {
    mount();
    await screen.findByText("plugins.empty");
    fireEvent.click(screen.getByRole("button", { name: "plugins.install" }));
    await screen.findByText("Test notes");
    expect(screen.getByText("plugins.open")).toBeDisabled();
    fireEvent.click(screen.getByText("plugins.enable"));
    expect(mocks.setEnabled).not.toHaveBeenCalled();
    fireEvent.click(screen.getAllByText("plugins.enable").at(-1)!);
    await waitFor(() => expect(screen.getByText("plugins.open")).toBeEnabled());
    fireEvent.click(screen.getByText("plugins.open"));
    await waitFor(() =>
      expect(mocks.startRuntime).toHaveBeenCalledWith(
        "test.notes",
        "notes",
        expect.any(Object),
      ),
    );
    expect(document.querySelector("iframe")).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "About" }));
    await waitFor(() =>
      expect(mocks.startRuntime).toHaveBeenCalledWith(
        "test.notes",
        "about",
        expect.any(Object),
      ),
    );
    expect(mocks.stopRuntime).not.toHaveBeenCalled();
    fireEvent.click(screen.getByLabelText("plugins.back"));
    expect(mocks.stopRuntime).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("plugins.open"));
    expect(mocks.startRuntime).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByLabelText("plugins.back"));
    fireEvent.click(screen.getByText("plugins.disable"));
    await waitFor(() =>
      expect(screen.getByText("plugins.open")).toBeDisabled(),
    );
    fireEvent.click(screen.getByLabelText("plugins.uninstallNamed"));
    expect(mocks.uninstall).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "plugins.uninstall" }));
    await screen.findByText("plugins.empty");
    expect(mocks.uninstall).toHaveBeenCalledWith("test.notes");
    expect(parseView("plugins")).toBe("plugins");
  });

  it("closes old page state after updating an installed plugin", async () => {
    state.plugins = [{ ...structuredClone(plugin), enabled: true }];
    mocks.install.mockImplementation(async () => {
      state.plugins[0].manifest.version = "2.0.0";
      return structuredClone(state.plugins[0]);
    });
    mount();
    await screen.findByText("Test notes");
    fireEvent.click(screen.getByText("plugins.open"));
    await waitFor(() => expect(mocks.startRuntime).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByLabelText("plugins.back"));
    fireEvent.click(screen.getByRole("button", { name: "plugins.install" }));
    await waitFor(() =>
      expect(mocks.stopRuntime).toHaveBeenCalledWith("session-1"),
    );
    await screen.findByText(/2\.0\.0/);
    fireEvent.click(screen.getByText("plugins.open"));
    await waitFor(() => expect(mocks.startRuntime).toHaveBeenCalledTimes(2));
  });

  it("cancellation performs no install; failed install leaves the list and exposes a retryable error", async () => {
    mocks.open.mockResolvedValueOnce(null);
    mount();
    await screen.findByText("plugins.empty");
    fireEvent.click(screen.getByRole("button", { name: "plugins.install" }));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "plugins.install" }),
      ).toBeEnabled(),
    );
    expect(mocks.install).not.toHaveBeenCalled();
    mocks.install.mockRejectedValueOnce(new Error("Invalid manifest"));
    fireEvent.click(screen.getByRole("button", { name: "plugins.install" }));
    await screen.findByText("Invalid manifest");
    expect(screen.getByText("plugins.empty")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "plugins.install" }));
    await screen.findByText("Test notes");
    expect(screen.queryByText("Invalid manifest")).not.toBeInTheDocument();
  });

  it("refreshes the list when uninstall partially succeeds", async () => {
    state.plugins = [structuredClone(plugin)];
    mocks.uninstall.mockImplementation(async () => {
      state.plugins = [];
      throw new Error("Cleanup failed");
    });
    mount();
    await screen.findByText("Test notes");
    fireEvent.click(screen.getByLabelText("plugins.uninstallNamed"));
    fireEvent.click(screen.getByRole("button", { name: "plugins.uninstall" }));
    await screen.findByText("plugins.empty");
    expect(screen.getAllByText(/Cleanup failed/).length).toBeGreaterThan(0);
  });

  it("shows corrupt entries alongside valid plugins", async () => {
    state.plugins = [plugin];
    state.problems = [{ id: "broken", error: "Invalid JSON" }];
    mount();
    await screen.findByText("Test notes");
    expect(screen.getByText("broken")).toBeInTheDocument();
    expect(screen.getByText("Invalid JSON")).toBeInTheDocument();
  });
});

describe("isolated plugin runtime", () => {
  const mountRuntime = () =>
    render(
      <PluginRuntime
        plugin={{ ...plugin, enabled: true }}
        page={plugin.manifest.pages[0]}
      />,
    );

  it("opens only a native runtime and keeps stop/focus controls outside plugin code", async () => {
    mountRuntime();
    await screen.findByText("plugins.runtime_running");
    expect(document.querySelector("iframe")).toBeNull();
    fireEvent.click(screen.getByText("plugins.showWindow"));
    await waitFor(() =>
      expect(mocks.updateRuntime).toHaveBeenCalledWith("session-1"),
    );
    fireEvent.click(screen.getByText("plugins.stopRuntime"));
    await screen.findByText("plugins.runtime_closed");
    expect(mocks.stopRuntime).toHaveBeenCalledWith("session-1");
  });

  it("restarts an unresponsive runtime with a new session and cleans up the old session", async () => {
    mocks.getRuntime.mockResolvedValueOnce({
      sessionId: "session-1",
      state: "unresponsive",
      pid: 123,
    });
    mountRuntime();
    await screen.findByText("plugins.runtime_unresponsive");
    fireEvent.click(screen.getByText("plugins.reopen"));
    await screen.findByText("plugins.runtime_running");
    expect(mocks.startRuntime).toHaveBeenCalledTimes(2);
    expect(mocks.stopRuntime).toHaveBeenCalledWith("session-1");
  });

  it("terminates a late startup result after leaving the plugin page", async () => {
    let finish!: (value: unknown) => void;
    mocks.startRuntime.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const view = mountRuntime();
    view.unmount();
    await act(async () =>
      finish({ sessionId: "late-session", state: "starting", pid: 123 }),
    );
    expect(mocks.stopRuntime).toHaveBeenCalledWith("late-session");
    expect(mocks.getRuntime).not.toHaveBeenCalled();
  });

  it("shows startup errors and permits retry without remounting the application", async () => {
    mocks.startRuntime.mockRejectedValueOnce(new Error("Cannot start process"));
    mountRuntime();
    await screen.findByText("Cannot start process");
    fireEvent.click(screen.getByText("plugins.reopen"));
    await screen.findByText("plugins.runtime_running");
    expect(mocks.startRuntime).toHaveBeenCalledTimes(2);
  });
});
