import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createContext, runInContext } from "node:vm";
import { describe, expect, it, vi } from "vitest";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";

const source = readFileSync(resolve("sdk/ui-plugin.js"), "utf8");

describe("UI plugin SDK", () => {
  it("connects, correlates results, rejects errors and ignores messages from other windows", async () => {
    const handlers = new Map<string, (event: any) => void>();
    const parent = { postMessage: vi.fn() };
    const dispatchEvent = vi.fn();
    const context = createContext({
      parent,
      addEventListener: (name: string, handler: (event: unknown) => void) =>
        handlers.set(name, handler),
      dispatchEvent,
      CustomEvent,
      setTimeout,
      clearTimeout,
    });
    runInContext("window = globalThis", context);
    runInContext(source, context);
    const api = context.CCSwitch;
    handlers.get("DOMContentLoaded")!({});
    const request = parent.postMessage.mock.calls[0][0];
    const reply = (data: object, source = parent) =>
      handlers.get("message")!({
        source,
        data: { channel: "cc-switch-plugin/v1", ...data },
      });
    const hostContext = {
      apiVersion: 1,
      language: "zh",
      theme: "dark",
      pluginId: "test",
      pageId: "notes",
    };
    reply(
      { requestId: request.requestId, value: "forged" },
      {} as typeof parent,
    );
    reply({ requestId: request.requestId, value: hostContext });
    expect(await api.ready).toEqual(hostContext);
    const write = api.setStorage({ note: "hi" });
    const writeRequest = parent.postMessage.mock.calls[1][0];
    expect(writeRequest.method).toBe("setStorage");
    reply({ requestId: writeRequest.requestId, value: null });
    expect(await write).toBeNull();
    const read = api.getStorage();
    const rejection = expect(read).rejects.toThrow("disabled");
    reply({
      requestId: parent.postMessage.mock.calls[2][0].requestId,
      error: "disabled",
    });
    await rejection;
    reply({
      event: "contextChanged",
      value: { ...hostContext, theme: "light" },
    });
    expect(dispatchEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        type: "cc-switch-context-changed",
        detail: expect.objectContaining({ theme: "light" }),
      }),
    );
  });

  it.each([zh, ja, zhTW])(
    "keeps plugin translations complete with matching placeholders",
    (locale) => {
      expect(Object.keys(locale.plugins).sort()).toEqual(
        Object.keys(en.plugins).sort(),
      );
      for (const key of Object.keys(
        en.plugins,
      ) as (keyof typeof en.plugins)[]) {
        expect(locale.plugins[key].trim()).not.toBe("");
        expect(locale.plugins[key].match(/\{\{.*?\}\}/g)).toEqual(
          en.plugins[key].match(/\{\{.*?\}\}/g),
        );
      }
    },
  );
});
