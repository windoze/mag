import type { StorybookConfig } from "@storybook/react-vite";

const config: StorybookConfig = {
  framework: {
    name: "@storybook/react-vite",
    options: {}
  },
  stories: ["../src/**/*.stories.@(ts|tsx)"],
  addons: [],
  viteFinal: (viteConfig) => ({
    ...viteConfig,
    build: {
      ...viteConfig.build,
      rollupOptions: {
        ...viteConfig.build?.rollupOptions,
        onwarn: (warning, defaultHandler) => {
          if (warning.code === "EVAL" && warning.id?.includes("@storybook/core") === true) {
            return;
          }
          defaultHandler(warning);
        }
      }
    }
  })
};

export default config;
