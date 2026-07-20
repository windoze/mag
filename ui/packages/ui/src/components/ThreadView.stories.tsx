import type { Meta, StoryObj } from "@storybook/react";

import { ThreadView } from "./ThreadView";
import type { ThreadItemView } from "./types";

const threadItems: ThreadItemView[] = [
  {
    type: "message",
    message: {
      id: "msg-user-1",
      role: "user",
      text: "Please inspect `src/main.rs` and explain the startup path.",
      attachments: [{ name: "notes.md", mime_type: "text/markdown", uri: "file:///tmp/notes.md" }]
    }
  },
  {
    type: "message",
    message: {
      id: "msg-assistant-1",
      role: "assistant",
      text: "## Startup path\n\n- Parse CLI flags\n- Load config\n- Start the selected interface\n\nStreaming **markdown** is rendered incrementally.",
      streaming: true
    }
  },
  {
    type: "tool_call",
    toolCall: {
      id: "tool-1",
      trace: {
        call_id: "tool-1",
        name: "read_file",
        input: { path: "crates/mag/src/main.rs" },
        output: { preview: "fn main() -> Result<()>" },
        status: "finished"
      }
    }
  },
  {
    type: "interaction",
    interaction: {
      requestId: "req-1",
      origin: { delegate: "reviewer", depth: 1 },
      kind: {
        kind: "choice",
        prompt: "Which follow-up should run next?",
        options: ["Focused unit tests", "Full frontend build", "Cargo workspace"]
      }
    }
  },
  {
    type: "delegation",
    delegation: {
      id: "run-1:reviewer",
      delegate: "reviewer",
      status: "started",
      task: "Audit interaction-card accessibility",
      usage: { total_tokens: 2048 }
    }
  },
  { type: "pivot", notice: { id: "pivot-1", status: "queued" } },
  {
    type: "run_error",
    error: {
      id: "err-1",
      kind: "budget_exhausted",
      message: "The configured token budget was reached."
    }
  }
];

const meta = {
  title: "Core/ThreadView",
  component: ThreadView,
  parameters: {
    layout: "fullscreen"
  },
  args: {
    items: threadItems,
    onRespondInteraction: () => undefined,
    onOpenDelegation: () => undefined
  }
} satisfies Meta<typeof ThreadView>;

export default meta;

type Story = StoryObj<typeof meta>;

export const StreamingThread: Story = {};

export const Empty: Story = {
  args: {
    items: []
  }
};

export const PivotAndErrors: Story = {
  args: {
    items: [
      { type: "pivot", notice: { id: "pivot-applied", status: "applied" } },
      {
        type: "pivot",
        notice: { id: "pivot-dropped", status: "dropped", reason: "run already completed" }
      },
      {
        type: "run_error",
        error: { id: "cancelled", kind: "cancelled", message: "Run cancelled by user." }
      },
      {
        type: "run_error",
        error: {
          id: "loop",
          kind: "loop_limit_exceeded",
          message: "The agent hit the configured loop limit."
        }
      },
      {
        type: "run_error",
        error: { id: "other", kind: "other", message: "Unexpected backend error." }
      }
    ]
  }
};
