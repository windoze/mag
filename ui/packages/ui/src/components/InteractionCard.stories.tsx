import type { Meta, StoryObj } from "@storybook/react";

import { InteractionCard } from "./InteractionCard";

const meta = {
  title: "Core/InteractionCard",
  component: InteractionCard,
  parameters: {
    layout: "centered"
  },
  args: {
    requestId: "req-approval",
    kind: {
      kind: "approval",
      call_id: "11111111-1111-1111-1111-111111111111",
      requirement: { type: "require_approval", reason: "writes outside the current worktree" },
      tool_name: "shell",
      input: { command: "rm -rf /tmp/mag-fixture" }
    }
  }
} satisfies Meta<typeof InteractionCard>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Approval: Story = {};

export const Question: Story = {
  args: {
    requestId: "req-question",
    kind: { kind: "question", prompt: "Which branch should receive this patch?" }
  }
};

export const Choice: Story = {
  args: {
    requestId: "req-choice",
    kind: {
      kind: "choice",
      prompt: "Pick a validation profile",
      options: ["Focused UI tests", "Frontend workspace", "Full repository"]
    }
  }
};

export const PermissionFromDelegate: Story = {
  args: {
    requestId: "req-permission",
    origin: { delegate: "reviewer", depth: 2 },
    kind: {
      kind: "permission",
      action_id: "perm-network-1",
      actor: "reviewer",
      category: "network",
      risk: "high",
      summary: "Open network connection",
      subject: { host: "example.test", port: 443 },
      reason: "Validate remote release metadata"
    }
  }
};

export const Resolved: Story = {
  args: {
    requestId: "req-resolved",
    status: "responded",
    kind: { kind: "choice", prompt: "Pick a target", options: ["web", "desktop"] },
    response: { kind: "choice", index: 0 }
  }
};
