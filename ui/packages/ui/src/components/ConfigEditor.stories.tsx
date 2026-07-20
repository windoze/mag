import type { Meta, StoryObj } from "@storybook/react";

import { ConfigEditor } from "./ConfigEditor";

const sampleToml = `[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "ANTHROPIC_API_KEY" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
tools = ["read_file", "grep"]

[session]
routing = "model_routed"
`;

const meta = {
  title: "Core/ConfigEditor",
  component: ConfigEditor,
  parameters: {
    layout: "fullscreen"
  },
  args: {
    text: sampleToml,
    onApply: () => undefined,
    onBack: () => undefined,
    onChange: () => undefined,
    onReload: () => undefined,
    onSave: () => undefined
  }
} satisfies Meta<typeof ConfigEditor>;

export default meta;

type Story = StoryObj<typeof meta>;

export const TextMode: Story = {};

export const Editing: Story = {
  args: {
    text: `${sampleToml}\n[approval]\ndefault_policy = "ask"\ntimeout_secs = 120\n`
  }
};

export const Saving: Story = {
  args: {
    busy: true
  }
};

export const SaveError: Story = {
  args: {
    error: "Invalid TOML: expected an equals, found a newline at line 4 column 12"
  }
};

export const GraphPlaceholder: Story = {
  args: {
    mode: "graph"
  }
};
