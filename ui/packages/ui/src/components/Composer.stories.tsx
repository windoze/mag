import type { Meta, StoryObj } from "@storybook/react";

import { Composer } from "./Composer";

const meta = {
  title: "Core/Composer",
  component: Composer,
  parameters: {
    layout: "centered"
  },
  args: {
    value: "Summarize the current change set.",
    onValueChange: () => undefined,
    onSend: () => undefined
  }
} satisfies Meta<typeof Composer>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Idle: Story = {};

export const RunningPivot: Story = {
  args: {
    mode: "running",
    value: "Pivot to a smaller test fixture.",
    onCancel: () => undefined
  }
};

export const PendingInteraction: Story = {
  args: {
    mode: "awaiting_interaction",
    pendingInteractionCount: 2,
    value: "Continue after approval."
  }
};

export const Disabled: Story = {
  args: {
    disabled: true,
    value: "Waiting for transport..."
  }
};
