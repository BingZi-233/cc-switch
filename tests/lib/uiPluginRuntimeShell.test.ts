import { readFileSync } from "node:fs";
import { createContext, runInContext } from "node:vm";
import { describe, expect, it, vi } from "vitest";

const shell = readFileSync(
  "src-tauri/src/ui_plugins/runtime/shell.html",
  "utf8",
).match(/<script>([\s\S]*?)<\/script>/)![1];

describe("plugin child shell", () => {
  it("binds requests and health replies to its sandbox and limits outstanding work", async () => {
    const handlers = new Map<string, (event: any) => Promise<void>>();
    const frame = {
      contentWindow: { postMessage: vi.fn() },
      src: "",
      title: "",
    };
    let complete!: (value: unknown) => void;
    const invoke = vi.fn().mockImplementation((method) =>
      method === "plugin_runtime_heartbeat"
        ? Promise.resolve()
        : new Promise((resolve) => {
            complete = resolve;
          }),
    );
    const context = createContext({
      document: { getElementById: () => frame, documentElement: { style: {} } },
      location: { href: "cc-plugin://localhost/shell" },
      __PLUGIN_CONTEXT__: {
        pluginId: "test.plugin",
        language: "zh",
        theme: "dark",
      },
      __TAURI_INTERNALS__: { invoke },
      addEventListener: (name: string, handler: any) =>
        handlers.set(name, handler),
    });
    runInContext("window = globalThis", context);
    runInContext(shell, context);
    expect(frame.src).toBe("cc-plugin://localhost/page");
    const send = (
      data: object,
      source: unknown = frame.contentWindow,
      origin = "null",
    ) =>
      handlers.get("message")!({
        source,
        origin,
        data: { channel: "cc-switch-plugin/v1", ...data },
      });
    const request = { requestId: "1", method: "getStorage" };
    await send(request, {});
    await send(request, frame.contentWindow, "https://other.example");
    expect(invoke).not.toHaveBeenCalled();
    const pending = send(request);
    await send({ ...request, requestId: "2" });
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(frame.contentWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        requestId: "2",
        error: expect.stringContaining("busy"),
      }),
      "*",
    );
    context.__pluginPing();
    const challenge =
      frame.contentWindow.postMessage.mock.lastCall![0].challenge;
    await send({ event: "healthReply", challenge });
    await send({ event: "healthReply", challenge });
    expect(
      invoke.mock.calls.filter(
        (call) => call[0] === "plugin_runtime_heartbeat",
      ),
    ).toHaveLength(1);
    complete({ note: "saved" });
    await pending;
    await send({ requestId: "3", method: "invoke" });
    expect(frame.contentWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        requestId: "3",
        error: "Unsupported plugin method",
      }),
      "*",
    );
  });
});
