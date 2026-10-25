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
import { PluginFrame, PLUGIN_CHANNEL } from "@/components/plugins/PluginFrame";
import type { InstalledPlugin, PluginList } from "@/lib/api/uiPlugins";
import { parseView } from "@/lib/navigation";

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  install: vi.fn(),
  setEnabled: vi.fn(),
  setPermissions: vi.fn(),
  getProviders: vi.fn(),
  getUsage: vi.fn(),
  uninstall: vi.fn(),
  getStorage: vi.fn(),
  setStorage: vi.fn(),
  open: vi.fn(),
}));
vi.mock("@/lib/api/uiPlugins", () => ({
  uiPluginsKey: ["ui-plugins"],
  uiPluginsApi: {
    ...mocks,
    pageUrl: (id: string, page: string) =>
      `http://cc-plugin.localhost/${id}/${page}`,
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
  mocks.getProviders.mockResolvedValue({ items: [], total: 0 });
  mocks.getUsage.mockRejectedValue("PLUGIN_PERMISSION_DENIED");
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
    ).toBeChecked();
    fireEvent.click(
      screen.getByRole("checkbox", { name: /plugins.usageRead / }),
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
    const original = screen.getByTitle("Test notes — Notes");
    fireEvent.click(screen.getByLabelText("plugins.permissionsNamed"));
    fireEvent.click(
      screen.getByRole("checkbox", { name: /plugins.providersRead / }),
    );
    fireEvent.click(screen.getByText("plugins.savePermissions"));
    await waitFor(() =>
      expect(mocks.setPermissions).toHaveBeenCalledWith("test.notes", []),
    );
    await waitFor(() =>
      expect(screen.getByTitle("Test notes — Notes")).not.toBe(original),
    );
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
    const frame = screen.getByTitle("Test notes — Notes");
    expect(frame).toHaveAttribute("sandbox", "allow-scripts");
    fireEvent.click(screen.getByRole("tab", { name: "About" }));
    expect(screen.getByTitle("Test notes — About")).toHaveAttribute(
      "src",
      "http://cc-plugin.localhost/test.notes/about",
    );
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

  it("shows corrupt entries alongside valid plugins", async () => {
    state.plugins = [plugin];
    state.problems = [{ id: "broken", error: "Invalid JSON" }];
    mount();
    await screen.findByText("Test notes");
    expect(screen.getByText("broken")).toBeInTheDocument();
    expect(screen.getByText("Invalid JSON")).toBeInTheDocument();
  });
});

describe("plugin frame boundary", () => {
  it("binds business reads to the mounted identity and returns backend denials", async () => {
    render(
      <PluginFrame
        plugin={{ ...plugin, enabled: true }}
        page={plugin.manifest.pages[0]}
      />,
    );
    const target = (
      screen.getByTitle("Test notes — Notes") as HTMLIFrameElement
    ).contentWindow!;
    const post = vi.spyOn(target, "postMessage");
    const send = (method: string) =>
      act(() => {
        window.dispatchEvent(
          new MessageEvent("message", {
            source: target,
            origin: "null",
            data: {
              channel: PLUGIN_CHANNEL,
              requestId: method,
              method,
              pluginId: "privileged.plugin",
              value: { appType: "codex" },
            },
          }),
        );
      });
    send("providers.list");
    await waitFor(() =>
      expect(post).toHaveBeenCalledWith(
        expect.objectContaining({
          requestId: "providers.list",
          value: expect.any(Object),
        }),
        "*",
      ),
    );
    expect(mocks.getProviders).toHaveBeenCalledWith("test.notes", {
      appType: "codex",
    });
    send("usage.getStats");
    await waitFor(() =>
      expect(post).toHaveBeenCalledWith(
        expect.objectContaining({ error: "PLUGIN_PERMISSION_DENIED" }),
        "*",
      ),
    );
    expect(mocks.getUsage).toHaveBeenCalledWith("test.notes", {
      appType: "codex",
    });
  });
  it("rejects other windows/origins and unknown methods; scopes writes to the mounted plugin", async () => {
    const result = render(
      <PluginFrame
        plugin={{ ...plugin, enabled: true }}
        page={plugin.manifest.pages[0]}
      />,
    );
    const frame = screen.getByTitle("Test notes — Notes") as HTMLIFrameElement;
    const target = frame.contentWindow!;
    const post = vi.spyOn(target, "postMessage");
    const send = (data: object, source: Window = target, origin = "null") =>
      act(() => {
        window.dispatchEvent(
          new MessageEvent("message", {
            source,
            origin,
            data: { channel: PLUGIN_CHANNEL, requestId: "1", ...data },
          }),
        );
      });
    send({ method: "setStorage", value: "bad" }, window);
    send({ method: "setStorage", value: "bad" }, target, "https://example.com");
    send({ method: "invoke", command: "get_providers" });
    expect(mocks.setStorage).not.toHaveBeenCalled();
    expect(post).toHaveBeenCalledWith(
      expect.objectContaining({ error: "Unsupported plugin method" }),
      "*",
    );
    send({
      method: "setStorage",
      pluginId: "other.plugin",
      value: { note: "new" },
    });
    await waitFor(() =>
      expect(mocks.setStorage).toHaveBeenCalledWith("test.notes", {
        note: "new",
      }),
    );
    await waitFor(() =>
      expect(post).toHaveBeenCalledWith(
        expect.objectContaining({ value: null }),
        "*",
      ),
    );
    send({ method: "setStorage", value: "x".repeat(65536) });
    expect(mocks.setStorage).toHaveBeenCalledTimes(1);
    result.unmount();
    send({ method: "setStorage", value: "late" });
    expect(mocks.setStorage).toHaveBeenCalledTimes(1);
  });

  it("bounds in-flight work and suppresses responses after the frame is unmounted", async () => {
    let resolve!: (value: unknown) => void;
    mocks.getStorage.mockReturnValue(
      new Promise((res) => {
        resolve = res;
      }),
    );
    const result = render(
      <PluginFrame
        plugin={{ ...plugin, enabled: true }}
        page={plugin.manifest.pages[0]}
      />,
    );
    const target = (
      screen.getByTitle("Test notes — Notes") as HTMLIFrameElement
    ).contentWindow!;
    const post = vi.spyOn(target, "postMessage");
    act(() => {
      for (const requestId of ["a", "b"])
        window.dispatchEvent(
          new MessageEvent("message", {
            source: target,
            origin: "null",
            data: { channel: PLUGIN_CHANNEL, requestId, method: "getStorage" },
          }),
        );
    });
    expect(mocks.getStorage).toHaveBeenCalledTimes(1);
    expect(post).toHaveBeenCalledWith(
      expect.objectContaining({
        requestId: "b",
        error: expect.stringContaining("busy"),
      }),
      "*",
    );
    post.mockClear();
    result.unmount();
    await act(async () => resolve({ secret: "stale" }));
    expect(post).not.toHaveBeenCalled();
  });
});
