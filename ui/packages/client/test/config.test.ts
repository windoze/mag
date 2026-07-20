import type { Command, ConfigDto, SourceInfo } from "@mag/protocol";
import { describe, expect, it, vi } from "vitest";

import { SessionStore, configDtoToToml, tomlToConfigDto, type ITransport } from "../src/index";

const sampleDto = {
  providers: {
    anthropic: {
      wire: "anthropic",
      base_url: "https://api.anthropic.com",
      api_key: { env: "ANTHROPIC_API_KEY" }
    }
  },
  agents: {
    default: {
      provider: "anthropic",
      model: "claude-sonnet-4-5",
      tools: ["read_file", "grep"]
    }
  },
  session: { routing: "model_routed" },
  approval: { default_policy: "ask", timeout_secs: 60 }
} satisfies ConfigDto;

const sampleSources = [
  {
    id: "claude-code",
    name: "Claude Code",
    kind: "local_agent",
    available: true,
    version: "1.2.3",
    capabilities: ["delegate"]
  }
] satisfies SourceInfo[];

describe("config TOML mapping", () => {
  it("round-trips a ConfigDto through TOML preserving data", () => {
    const text = configDtoToToml(sampleDto);
    expect(text).toContain("[providers.anthropic]");
    expect(text).toContain('env = "ANTHROPIC_API_KEY"');
    expect(tomlToConfigDto(text)).toEqual(sampleDto);
  });

  it("serializes an empty DTO to empty text", () => {
    expect(configDtoToToml({})).toBe("");
  });

  it("strips undefined and null values before serializing", () => {
    const dto = {
      providers: {
        anthropic: { wire: "anthropic", base_url: undefined, api_key: null }
      },
      agents: undefined
    } as unknown as ConfigDto;

    const text = configDtoToToml(dto);
    expect(text).not.toContain("base_url");
    expect(text).not.toContain("api_key");
    expect(text).not.toContain("agents");
    expect(tomlToConfigDto(text)).toEqual({ providers: { anthropic: { wire: "anthropic" } } });
  });

  it("maps empty and whitespace-only text to an empty DTO", () => {
    expect(tomlToConfigDto("")).toEqual({});
    expect(tomlToConfigDto("  \n\t\n")).toEqual({});
  });

  it("throws a clear error on invalid TOML", () => {
    expect(() => tomlToConfigDto("[providers")).toThrowError(/^Invalid TOML:/);
    expect(() => tomlToConfigDto("= 1")).toThrowError(/^Invalid TOML:/);
  });

  it("keeps secret references as plain inline tables", () => {
    const text = `[providers.anthropic]\napi_key = { keyring = "mag/anthropic" }\n`;
    expect(tomlToConfigDto(text)).toEqual({
      providers: { anthropic: { api_key: { keyring: "mag/anthropic" } } }
    });
  });
});

describe("SessionStore config and sources helpers", () => {
  it("getConfigText sends get_config and returns TOML text", async () => {
    const transport = new StubTransport(() => sampleDto);
    const store = new SessionStore(transport);

    const text = await store.getConfigText();

    expect(transport.sent).toEqual([{ type: "get_config" }]);
    expect(text).toContain("[providers.anthropic]");
    expect(tomlToConfigDto(text)).toEqual(sampleDto);
  });

  it("saveConfigText parses TOML and sends update_config with the DTO payload", async () => {
    const transport = new StubTransport(() => undefined);
    const store = new SessionStore(transport);

    await store.saveConfigText(configDtoToToml(sampleDto));

    expect(transport.sent).toEqual([{ type: "update_config", config: sampleDto }]);
  });

  it("saveConfigText rejects invalid TOML without sending a command", async () => {
    const transport = new StubTransport(() => undefined);
    const store = new SessionStore(transport);

    await expect(store.saveConfigText("[oops")).rejects.toThrowError(/^Invalid TOML:/);
    expect(transport.sent).toEqual([]);
  });

  it("reloadConfig and applyConfig send their commands", async () => {
    const transport = new StubTransport(() => undefined);
    const store = new SessionStore(transport);

    await store.reloadConfig();
    await store.applyConfig();

    expect(transport.sent).toEqual([{ type: "reload_config" }, { type: "apply_config" }]);
  });

  it("refreshSources replaces snapshot sources and notifies", async () => {
    const transport = new StubTransport(() => sampleSources);
    const store = new SessionStore(transport);
    const listener = vi.fn();
    store.subscribe(listener);

    const sources = await store.refreshSources();

    expect(transport.sent).toEqual([{ type: "list_sources" }]);
    expect(sources).toEqual(sampleSources);
    expect(store.getSnapshot().sources).toEqual(sampleSources);
    expect(listener).toHaveBeenCalled();
  });

  it("probeSources replaces snapshot sources with the probed list", async () => {
    const probed = [
      { ...sampleSources[0], available: false, version: undefined }
    ] satisfies SourceInfo[];
    const transport = new StubTransport((command) =>
      command.type === "probe_local_agents" ? probed : sampleSources
    );
    const store = new SessionStore(transport);

    await store.refreshSources();
    const sources = await store.probeSources();

    expect(transport.sent.map((command) => command.type)).toEqual([
      "list_sources",
      "probe_local_agents"
    ]);
    expect(sources).toEqual(probed);
    expect(store.getSnapshot().sources).toEqual(probed);
  });
});

class StubTransport implements ITransport {
  readonly kind = "web" as const;
  readonly sent: Command[] = [];

  constructor(private readonly responder: (command: Command) => unknown) {}

  async send(command: Command): Promise<unknown> {
    this.sent.push(command);
    return this.responder(command);
  }

  subscribe(): () => void {
    return () => undefined;
  }
}
