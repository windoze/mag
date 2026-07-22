import type { Command, ConfigDto, Event, HistoryEntry, SessionInfo } from "../src/index";
import updateConfigCommandJson from "./fixtures/update-config-command.json";

const sessionId = "018f0d9c-7b6a-7c12-8f31-000000000001";
const runId = "018f0d9c-7b6a-7c12-8f31-000000000003";
const requestId = "018f0d9c-7b6a-7c12-8f31-000000000002";

export const updateConfigCommandJsonFixture = updateConfigCommandJson as Command;

export const configFixture = {
  providers: {
    anthropic: {
      wire: "anthropic",
      api_key: { env: "ANTHROPIC_API_KEY" }
    }
  },
  agents: {
    default: {
      provider: "anthropic",
      model: "claude-sonnet-4-5",
      tools: ["read_file", "shell"]
    }
  },
  session: {
    routing: "model_routed",
    budget: { max_steps: 8 }
  },
  approval: {
    default_policy: "ask",
    timeout_secs: 60
  }
} satisfies ConfigDto;

export const updateConfigCommand = {
  type: "update_config",
  config: configFixture
} satisfies Command;

export const interactionEvent = {
  type: "interaction_requested",
  id: sessionId,
  request_id: requestId,
  kind: {
    kind: "question",
    prompt: "Proceed?"
  },
  origin: {
    depth: 0
  }
} satisfies Event;

export const historyEntry = {
  type: "tool_call",
  trace: {
    run_id: runId,
    call_id: "018f0d9c-7b6a-7c12-8f31-000000000006",
    name: "read_file",
    input: { path: "README.md" },
    output: { bytes: 10 },
    status: "finished"
  }
} satisfies HistoryEntry;

export const sessionInfo = {
  id: sessionId,
  agent: "default",
  cwd: "/work/session-root",
  title: "Inspect README",
  last_active_at: 1721234567890,
  status: "idle"
} satisfies SessionInfo;
