import type { Meta, StoryObj } from "@storybook/react";

import { SourcesView } from "./SourcesView";

const meta = {
  title: "Core/SourcesView",
  component: SourcesView,
  parameters: {
    layout: "fullscreen"
  },
  args: {
    sources: [
      {
        id: "anthropic",
        name: "Anthropic",
        kind: "llm_provider",
        available: true,
        version: "claude-sonnet-4-5",
        capabilities: ["chat", "tools", "streaming"]
      },
      {
        id: "claude-code",
        name: "Claude Code",
        kind: "local_agent",
        available: true,
        version: "1.2.3",
        capabilities: ["delegate"]
      },
      {
        id: "ollama",
        name: "Ollama",
        kind: "local_agent",
        available: false,
        capabilities: []
      }
    ],
    onBack: () => undefined,
    onProbe: () => undefined
  }
} satisfies Meta<typeof SourcesView>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Populated: Story = {};

export const Empty: Story = {
  args: {
    sources: []
  }
};

export const Probing: Story = {
  args: {
    busy: true
  }
};

export const ProbeFailed: Story = {
  args: {
    error: "probe failed: backend unavailable"
  }
};
