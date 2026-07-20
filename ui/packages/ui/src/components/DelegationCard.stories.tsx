import type { Meta, StoryObj } from "@storybook/react";

import { DelegationCard } from "./DelegationCard";

const meta = {
  title: "Core/DelegationCard",
  component: DelegationCard,
  parameters: {
    layout: "centered"
  },
  args: {
    delegation: {
      id: "run-live:researcher",
      delegate: "researcher",
      status: "started",
      task: "look up context"
    }
  }
} satisfies Meta<typeof DelegationCard>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Started: Story = {};

export const FinishedWithUsage: Story = {
  args: {
    delegation: {
      id: "run-live:researcher",
      delegate: "researcher",
      status: "finished",
      task: "look up context",
      output: "context found",
      usage: { input_tokens: 1200, output_tokens: 340, total_tokens: 1540 }
    }
  }
};

export const Failed: Story = {
  args: {
    delegation: {
      id: "run-live:researcher",
      delegate: "researcher",
      status: "failed",
      task: "look up context",
      message: "delegate exited before producing output"
    }
  }
};

export const ClickableDrillDown: Story = {
  args: {
    delegation: {
      id: "run-live:reviewer",
      delegate: "reviewer",
      status: "finished",
      task: "review the diff",
      output: "lgtm",
      usage: { input_tokens: 800, output_tokens: 120, total_tokens: 920 }
    },
    onOpen: () => undefined
  },
  render: (args) => (
    <div className="w-[42rem]">
      <DelegationCard {...args} />
    </div>
  )
};
