import type { StorybookConfig } from "@storybook/react-vite";

const config: StorybookConfig = {
  stories: ["../src/**/*.stories.@(ts|tsx)"],
  framework: "@storybook/react-vite",
  addons: ["@storybook/addon-docs", "@storybook/addon-a11y"],
  typescript: {
    reactDocgen: "react-docgen-typescript",
    reactDocgenTypescriptOptions: {
      docgenMode: "legacy",
      shouldExtractLiteralValuesFromEnum: true,
      shouldRemoveUndefinedFromOptional: true,
    },
  },
  core: { disableTelemetry: true },
  async viteFinal(config) {
    config.plugins ??= [];
    config.plugins.push({
      name: "wizard:storybook-docgen-virtual-hmr",
      configResolved(resolved) {
        // Storybook emits HMR for its virtual story index. Docgen 0.9.1 reads
        // every update before filtering; null-prefixed IDs are never files.
        // Remove this guard when upstream filters virtual updates itself.
        const docgen = resolved.plugins.find(
          (plugin) => plugin.name === "vite:react-docgen-typescript",
        );
        const hotUpdate = docgen?.hotUpdate;
        if (docgen && typeof hotUpdate === "function") {
          docgen.hotUpdate = function (context) {
            if (context.file.includes("\0")) return;
            return hotUpdate.call(this, context);
          };
        }
      },
    });
    // Tauri's dev host/HMR ports are specific to the desktop application.
    config.server = {
      ...config.server,
      host: "127.0.0.1",
      hmr: undefined,
      port: undefined,
    };
    return config;
  },
};
export default config;
