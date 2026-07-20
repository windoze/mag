import type { Meta, StoryObj } from "@storybook/react";

import { DelegateThreadPanel } from "./DelegateThreadPanel";

const meta = {
  title: "Core/DelegateThreadPanel",
  component: DelegateThreadPanel,
  parameters: {
    layout: "centered"
  },
  args: {
    delegate: "researcher",
    depth: 1,
    status: "started",
    items: []
  }
} satisfies Meta<typeof DelegateThreadPanel>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Empty: Story = {
  render: (args) => (
    <div className="w-80">
      <DelegateThreadPanel {...args} />
    </div>
  )
};

export const WithActivity: Story = {
  args: {
    usage: { input_tokens: 1200, output_tokens: 340, total_tokens: 1540 },
    items: [
      { type: "message", message: { id: "m1", text: "Scanning the repository for context…" } },
      {
        type: "interaction",
        interaction: {
          requestId: "req-research",
          kind: {
            kind: "approval",
            call_id: "call-shell-1",
            requirement: { type: "require_approval", reason: "runs a shell command" },
            tool_name: "shell",
            input: { cmd: "rg TODO" }
          },
          origin: { delegate: "researcher", depth: 1 },
          status: "pending"
        }
      },
      { type: "message", message: { id: "m2", text: "Found 3 candidate files." } }
    ]
  },
  render: (args) => (
    <div className="w-96">
      <DelegateThreadPanel {...args} onClose={() => undefined} />
    </div>
  )
};

export const NestedDelegate: Story = {
  args: {
    delegate: "reviewer",
    depth: 2,
    status: "finished",
    usage: { input_tokens: 800, output_tokens: 120, total_tokens: 920 },
    items: [
      {
        type: "message",
        message: { id: "m1", text: "Reviewing the diff produced by researcher." }
      },
      {
        type: "interaction",
        interaction: {
          requestId: "req-review",
          kind: { kind: "question", prompt: "Approve the refactor plan?" },
          origin: { delegate: "reviewer", depth: 2 },
          status: "responded",
          response: { kind: "answer", text: "yes" }
        }
      }
    ]
  },
  render: (args) => (
    <div className="w-96">
      <DelegateThreadPanel {...args} onClose={() => undefined} />
    </div>
  )
};
