import type { Meta, StoryObj } from "@storybook/react";

import { ToolCallCard } from "./ToolCallCard";
import type { ToolStatusView } from "./types";

const meta = {
  title: "Core/ToolCallCard",
  component: ToolCallCard,
  parameters: {
    layout: "centered"
  },
  args: {
    trace: {
      call_id: "call-read-1",
      name: "read_file",
      input: { path: "src/main.rs" },
      output: { lines: ["fn main() {}"] },
      status: "finished"
    },
    defaultExpanded: true
  }
} satisfies Meta<typeof ToolCallCard>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Finished: Story = {};

export const AllStatuses: Story = {
  render: () => (
    <div className="grid w-[42rem] gap-3">
      {(["started", "finished", "denied", "cancelled", "failed"] satisfies ToolStatusView[]).map(
        (status) => (
          <ToolCallCard
            key={status}
            trace={{
              call_id: `call-${status}`,
              name: status === "started" ? "shell" : "read_file",
              input: { command: "cargo test -p mag-ui", path: "README.md" },
              output: status === "started" ? undefined : { status, bytes: 4096 },
              status,
              message: status === "failed" ? "command exited with status 1" : undefined
            }}
          />
        )
      )}
    </div>
  )
};

export const FromDelegate: Story = {
  args: {
    origin: { delegate: "codex", depth: 1 },
    trace: {
      call_id: "call-delegate-1",
      name: "grep",
      input: { pattern: "InteractionCard", path: "ui/packages/ui/src" },
      output: { matches: 7 },
      status: "finished"
    }
  }
};
