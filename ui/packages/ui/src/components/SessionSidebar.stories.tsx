import type { Meta, StoryObj } from "@storybook/react";

import { SessionSidebar } from "./SessionSidebar";

const now = Date.now();

const meta = {
  title: "Core/SessionSidebar",
  component: SessionSidebar,
  parameters: {
    layout: "fullscreen"
  },
  args: {
    activeSessionId: "sess-running",
    sessions: [
      {
        id: "sess-running",
        title: "Implement web UI",
        cwd: "/repos/mag",
        lastActiveAt: now - 90_000,
        status: "running",
        subtitle: "openai / gpt-5.5"
      },
      {
        id: "sess-awaiting",
        title: "Review approval gate",
        cwd: "/repos/mag",
        lastActiveAt: now - 12 * 60_000,
        status: "awaiting_interaction",
        subtitle: "local / llama"
      },
      {
        id: "sess-idle",
        title: "Archive docs cleanup",
        cwd: "/repos/docs",
        lastActiveAt: now - 3 * 60 * 60_000,
        status: "idle",
        subtitle: "anthropic / claude"
      }
    ],
    onNewSession: () => undefined,
    onOpenConfig: () => undefined,
    onOpenSources: () => undefined,
    onSelectSession: () => undefined,
    onDeleteSession: () => undefined
  }
} satisfies Meta<typeof SessionSidebar>;

export default meta;

type Story = StoryObj<typeof meta>;

export const GroupedSessions: Story = {};

export const Empty: Story = {
  args: {
    activeSessionId: undefined,
    sessions: []
  }
};
