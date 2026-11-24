import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within, waitFor } from "storybook/test";
import App from "../App";
import { LogWindow } from "../logs/LogWindow";

const meta = {
  title: "Экраны/Приложение",
  component: App,
  parameters: { layout: "fullscreen" },
  tags: ["!autodocs"],
} satisfies Meta<typeof App>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Overview: Story = { name: "Обзор" };
export const ConfiguredOverview: Story = {
  name: "Обзор · настроенные агенты",
  parameters: { scenario: "all_agents" },
};
export const FirstSetup: Story = {
  name: "Обзор · первое подключение",
  parameters: { scenario: "unconfigured" },
};
export const SystemProxyBanner: Story = {
  name: "Обзор · включить системный прокси",
  parameters: { scenario: "unconfigured" },
  play: async ({ canvas }) => {
    const banner = within(await canvas.findByRole("region", { name: /Для доступа к OpenAI и Anthropic из браузера/ }));
    await expect(banner.getByText("Все остальные ресурсы будут доступны напрямую.")).toBeVisible();
    await expect(banner.getByRole("button", { name: "Пропустить" })).toBeVisible();
    await expect(banner.getByRole("button", { name: "Включить" })).toBeEnabled();
  },
};
export const EmptyBalance: Story = {
  name: "Агент готов · пополнение хаба",
  parameters: { scenario: "balance_empty" },
};
export const GuestHub: Story = {
  name: "Гость · без баланса",
  parameters: { scenario: "balance_guest" },
};
export const UnavailableBalance: Story = {
  name: "Баланс недоступен",
  parameters: { scenario: "balance_error" },
};
export const OverviewTransformation: Story = {
  name: "Онбординг → две карточки",
  parameters: { scenario: "onboarding_merge" },
};
export const MissingCLI: Story = {
  name: "Установка CLI-агента",
  parameters: { scenario: "missing", route: "/agents/claude" },
};
export const SignedOut: Story = {
  name: "Без аккаунта",
  parameters: { scenario: "offline" },
};
export const PendingSignIn: Story = {
  name: "Ожидание входа",
  parameters: { scenario: "pending_login" },
};
export const FirstLaunch: Story = {
  name: "Первый запуск",
  parameters: { scenario: "startup" },
  play: async ({ canvas }) => {
    await expect(
      await canvas.findByRole(
        "heading",
        { name: "Ура! Wizard установлен." },
        { timeout: 5000 },
      ),
    ).toBeVisible();
  },
};
export const ClaudeDesktop: Story = {
  name: "Claude Desktop",
  parameters: { route: "/agents/claude-desktop" },
};
export const ChatGPT: Story = {
  name: "ChatGPT и Codex",
  parameters: { route: "/agents/codex" },
};
export const AdditionalModels: Story = {
  name: "ChatGPT и Codex · дополнительные модели",
  parameters: { scenario: "current", route: "/agents/codex" },
  play: async ({ canvas }) => {
    const panel = await canvas.findByRole("region", { name: "Дополнительные модели" });
    const toggle = await within(panel).findByRole("switch", { name: "Подключить coreinfra-media для видео-моделей" });
    await expect(toggle).toBeChecked();
    panel.scrollIntoView({ block: "center" });
  },
};
export const ClaudeCode: Story = {
  name: "Claude Code",
  parameters: { scenario: "all_agents", route: "/agents/claude" },
};
export const OpenCode: Story = {
  name: "OpenCode",
  parameters: { scenario: "all_agents", route: "/agents/opencode" },
};
export const Pi: Story = {
  name: "Pi",
  parameters: { scenario: "all_agents", route: "/agents/pi" },
};
export const Missing: Story = {
  name: "Неустановленный агент",
  parameters: { scenario: "missing", route: "/agents/claude-desktop" },
};
export const DetectionError: Story = {
  name: "Ошибка обнаружения",
  parameters: { scenario: "agents_error", route: "/agents/claude-desktop" },
};
export const ChatGPTDetectionError: Story = {
  name: "Обзор · ошибка обнаружения ChatGPT",
  parameters: { scenario: "chatgpt_error" },
};
export const DisabledConnection: Story = {
  name: "Claude · отключённое подключение",
  parameters: { scenario: "unconfigured", route: "/agents/claude-desktop" },
};
export const Checking: Story = {
  name: "Проверка обновлений в фоне",
  parameters: { scenario: "checking", route: "/agents/claude-desktop" },
};
export const WizardUpdate: Story = {
  name: "Обновление Wizard · окно установки",
  parameters: { scenario: "wizard_update" },
};
export const Current: Story = {
  name: "Актуальная версия",
  parameters: { scenario: "current", route: "/agents/claude-desktop" },
};
export const Downloads: Story = {
  name: "Загрузки в панели",
  parameters: { scenario: "downloads" },
};
export const NarrowMixedDownloads: Story = {
  name: "Загрузки · одна завершена",
  parameters: { scenario: "mixed_progress" },
  globals: { viewport: { value: "narrow", isRotated: false } },
};
export const ReadyVersion: Story = {
  name: "Версия готова к установке",
  parameters: { scenario: "ready", route: "/agents/claude-desktop" },
};
export const ReadyChatGPT: Story = {
  name: "ChatGPT · версия готова",
  parameters: { scenario: "ready", route: "/agents/codex" },
};
export const FailedVersionCheck: Story = {
  name: "Ошибка проверки версии",
  parameters: { scenario: "check_error", route: "/agents/claude-desktop" },
};
export const LongContent: Story = {
  name: "Длинные названия",
  parameters: { scenario: "long_content", route: "/agents/claude-desktop" },
};
export const Settings: Story = {
  name: "Настройки",
  parameters: { route: "/settings" },
};
export const SettingsFailure: Story = {
  name: "Ошибка настройки",
  parameters: { scenario: "settings_error", route: "/settings" },
  play: async ({ canvas }) => {
    await userEvent.click(
      await canvas.findByRole("switch", { name: "Работать в трее" }),
    );
    await expect(
      await canvas.findByText(
        "Не удалось сохранить настройку. Попробуйте ещё раз.",
      ),
    ).toBeVisible();
  },
};
export const ProxyConflict: Story = {
  name: "Конфликт системного прокси",
  parameters: { scenario: "proxy_conflict", route: "/settings" },
};
export const Unsupported: Story = {
  name: "Недоступная платформа",
  parameters: { scenario: "unsupported", route: "/settings" },
};
export const AccountMenu: Story = {
  name: "Меню аккаунта",
  play: async ({ canvas, canvasElement }) => {
    const page = within(canvasElement.ownerDocument.body);
    const menu = canvas.queryByRole("button", { name: "Открыть меню" });
    if (menu) await userEvent.click(menu);
    await userEvent.click(
      await page.findByRole("button", { name: /Меню аккаунта/ }),
    );
    await waitFor(() => expect(page.getByRole("menu")).toBeVisible());
  },
};
export const Logs: Story = { name: "Логи", render: () => <LogWindow /> };
export const NarrowOverview: Story = {
  name: "Обзор · узкое окно",
  globals: { viewport: { value: "narrow", isRotated: false } },
};
export const NarrowSettings: Story = {
  name: "Настройки · узкое окно",
  parameters: { route: "/settings" },
  globals: { viewport: { value: "narrow", isRotated: false } },
};
export const DarkDesktop: Story = {
  name: "Claude · тёмная тема",
  parameters: { route: "/agents/claude-desktop" },
  globals: { theme: "dark", viewport: { value: "desktop", isRotated: false } },
};
