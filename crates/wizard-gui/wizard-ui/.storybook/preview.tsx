import type { Preview } from "@storybook/react-vite";
import { useLayoutEffect, type ReactNode } from "react";
import { MotionConfig } from "motion/react";
import { startPreviewMocks, type Scenario } from "../src/preview/mocks";
import { initializeTheme, setTheme } from "../src/settings/theme";
import { SubscriptionTokenPreviewContext } from "../src/agents/connection_preview";
import "../src/styles.css";
import "../src/components/BrandIcons.css";

function Appearance({
  children,
  theme,
  motion,
}: {
  children: ReactNode;
  theme: "light" | "dark";
  motion: string;
}) {
  useLayoutEffect(() => {
    // Different preview tabs must keep their selected light/dark specimen.
    initializeTheme({ syncStorage: false });
    setTheme(theme);
    document.documentElement.dataset.motion = motion;
  }, [theme, motion]);
  return (
    <MotionConfig reducedMotion={motion === "reduce" ? "always" : "user"}>
      {children}
    </MotionConfig>
  );
}

const preview: Preview = {
  globalTypes: {
    theme: {
      description: "Тема",
      toolbar: {
        icon: "paintbrush",
        dynamicTitle: true,
        items: [
          { value: "light", title: "Светлая" },
          { value: "dark", title: "Тёмная" },
        ],
      },
    },
    motion: {
      description: "Движение",
      toolbar: {
        icon: "play",
        dynamicTitle: true,
        items: [
          { value: "system", title: "Анимации" },
          { value: "reduce", title: "Без движения" },
        ],
      },
    },
  },
  initialGlobals: { theme: "light", motion: "system" },
  parameters: {
    layout: "padded",
    controls: {
      expanded: true,
      matchers: { color: /(background|color)$/i, date: /Date$/i },
    },
    a11y: { test: "error" },
    options: {
      storySort: {
        order: ["Начало", "Catalyst", "Блоки", "Экраны", "Сценарии"],
      },
    },
    viewport: {
      options: {
        narrow: {
          name: "Узкий · 360",
          styles: { width: "360px", height: "800px" },
          type: "mobile",
        },
        small: {
          name: "Малый · 640",
          styles: { width: "640px", height: "800px" },
          type: "tablet",
        },
        medium: {
          name: "Средний · 800",
          styles: { width: "800px", height: "720px" },
          type: "tablet",
        },
        desktop: {
          name: "Wizard · 1100",
          styles: { width: "1100px", height: "720px" },
          type: "desktop",
        },
        wide: {
          name: "Широкий · 1440",
          styles: { width: "1440px", height: "900px" },
          type: "desktop",
        },
      },
    },
  },
  beforeEach(context) {
    const scenario = (context.parameters.scenario ?? "update") as Scenario;
    if (scenario !== "startup")
      localStorage.setItem("coreinfra.startup.seen", "true");
    const environment = startPreviewMocks(scenario, { onboarding: context.parameters.onboarding });
    window.location.hash = context.parameters.route ?? "/overview";
    return () => environment.dispose();
  },
  decorators: [
    (Story, context) => (
      <Appearance theme={context.globals.theme} motion={context.globals.motion}>
        <SubscriptionTokenPreviewContext.Provider value={true}>
          <Story />
        </SubscriptionTokenPreviewContext.Provider>
      </Appearance>
    ),
  ],
};
export default preview;
