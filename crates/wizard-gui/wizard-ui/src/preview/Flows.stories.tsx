import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within, waitFor } from "storybook/test";
import App from "../App";
import { finishDownloads } from "./mocks";

const meta = {
  title: "Сценарии/Переходы",
  component: App,
  parameters: { layout: "fullscreen" },
  tags: ["!autodocs"],
} satisfies Meta<typeof App>;
export default meta;
type Story = StoryObj<typeof meta>;

// Elements mount before the dialog's enter animation makes them visible.
async function expectVisible(element: Promise<HTMLElement>) {
  const target = await element;
  await waitFor(() => expect(target).toBeVisible());
}

async function openPicker(canvas: ReturnType<typeof within>, page: ReturnType<typeof within>) {
  await userEvent.click(await canvas.findByRole("button", { name: "Установить агента" }));
  return page.findByRole("dialog", { name: "Установите агентов" });
}

export const Picker: Story = {
  name: "Выбор приложения",
  parameters: { onboarding: true },
  play: async ({ canvas, canvasElement }) => {
    await openPicker(canvas, within(canvasElement.ownerDocument.body));
    await expectVisible(
      within(canvasElement.ownerDocument.body).findByRole("dialog", {
        name: "Установите агентов",
      }),
    );
  },
};
export const DownloadAndCancel: Story = {
  name: "Скачать → назад → свернуть → отменить",
  parameters: { route: "/overview", onboarding: true },
  play: async ({ canvas, canvasElement, step }) => {
    const page = within(canvasElement.ownerDocument.body);
    await step("Начать скачивание", async () => {
      const picker = await openPicker(canvas, page);
      await userEvent.click(within(picker).getAllByRole("button", { name: "Обновить" })[0]);
      const installation = await page.findByRole("dialog", { name: "Установка агента" });
      await userEvent.click(within(installation).getByRole("button", { name: "Скачать обновление" }));
      await expectVisible(page.findByRole("button", { name: "Отменить" }));
    });
    await step("Вернуться к выбору, сохранив загрузку", async () => {
      await userEvent.click(page.getByRole("button", { name: "Назад" }));
      const dialog = await page.findByRole("dialog", {
        name: "Установите агентов",
      });
      await expect(
        within(dialog).getByRole("progressbar", {
          name: "Скачивание Claude Desktop",
        }),
      ).toBeInTheDocument();
    });
    await step("Свернуть и открыть тот же процесс", async () => {
      await userEvent.click(
        page.getByRole("button", { name: "Свернуть установку" }),
      );
      await waitFor(() =>
        expect(page.queryByRole("dialog")).not.toBeInTheDocument(),
      );
      const menu = canvas.queryByRole("button", { name: "Открыть меню" });
      if (menu) await userEvent.click(menu);
      const navigation = menu
        ? within(await page.findByRole("dialog", { name: "Навигация" }))
        : page;
      await userEvent.click(
        await navigation.findByRole("button", {
          name: "Открыть установку Claude Desktop",
        }),
      );
      await expectVisible(
        page.findByRole("dialog", { name: "Установка агента" }),
      );
      await expect(within(page.getByRole("dialog", { name: "Установка агента" })).queryByRole("button", { name: "Назад" })).not.toBeInTheDocument();
      await waitFor(() =>
        expect(
          page
            .getByRole("dialog", { name: "Установка агента" })
            .contains(canvasElement.ownerDocument.activeElement),
        ).toBe(true),
      );
    });
    await step("Отменить скачивание", async () => {
      await userEvent.click(page.getByRole("button", { name: "Отменить" }));
      const dialog = await page.findByRole("dialog", {
        name: "Установка агента",
      });
      await expectVisible(
        within(dialog).findByRole("button", {
          name: "Скачать обновление",
        }),
      );
      await waitFor(() =>
        expect(dialog.contains(canvasElement.ownerDocument.activeElement)).toBe(
          true,
        ),
      );
    });
  },
};
export const NarrowDownloadAndCancel: Story = {
  ...DownloadAndCancel,
  name: "Узкое окно · навигация → загрузка → отмена",
  globals: { viewport: { value: "narrow", isRotated: false } },
};
export const NarrowActivity: Story = {
  name: "Узкое окно · загрузки в левой панели",
  parameters: { scenario: "downloads" },
  globals: { viewport: { value: "medium", isRotated: false } },
  play: async ({ canvas, canvasElement }) => {
    const page = within(canvasElement.ownerDocument.body);
    await expect(canvas.queryByRole("button", { name: /^Открыть загрузки/ })).not.toBeInTheDocument();
    await userEvent.click(await canvas.findByRole("button", { name: "Открыть меню" }));
    let navigation = await page.findByRole("dialog", { name: "Навигация" });
    await userEvent.click(within(navigation).getByRole("button", { name: "Открыть установку Claude Desktop" }));
    const dialog = await page.findByRole("dialog", { name: "Установка агента" });
    await expect(within(dialog).getAllByRole("progressbar")).toHaveLength(1);
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(page.queryByRole("dialog")).not.toBeInTheDocument());
    await userEvent.click(canvas.getByRole("button", { name: "Открыть меню" }));
    navigation = await page.findByRole("dialog", { name: "Навигация" });
    await expectVisible(within(navigation).findByRole("button", { name: "Открыть установку Claude Desktop" }));
    await expectVisible(within(navigation).findByRole("button", { name: "Открыть установку ChatGPT" }));
  },
};
export const ConnectionEditor: Story = {
  name: "Подключение · применение только при изменениях",
  parameters: { route: "/agents/claude-desktop" },
  play: async ({ canvas }) => {
    const apply = await canvas.findByRole("button", { name: "Применить" });
    await expect(apply).toBeDisabled();
    await expect(canvas.queryByRole("button", { name: "Отмена" })).not.toBeInTheDocument();
    await userEvent.click(canvas.getByRole("radio", { name: "Без CoreInfra AI Hub" }));
    await expect(apply).toBeEnabled();
    await userEvent.click(canvas.getByRole("radio", { name: "API-доступ" }));
    await expect(apply).toBeDisabled();
    await expect(canvas.getByRole("radio", { name: "API-доступ" })).toBeChecked();
  },
};

async function openClaudeCodePlan(canvas: ReturnType<typeof within>, page: ReturnType<typeof within>) {
  await userEvent.click(await canvas.findByRole("radio", { name: "Своя подписка Claude" }));
  const proxy = await page.findByRole("dialog", { name: "Включить системный прокси?" });
  await userEvent.click(within(proxy).getByRole("button", { name: "Пропустить" }));
  await userEvent.click(await canvas.findByRole("switch", { name: /coreinfra-media/ }));
  await userEvent.click(canvas.getByRole("button", { name: "Применить" }));
  return within(await page.findByRole("dialog", { name: "План изменений: Claude Code" }));
}

export const AgentChangePlan: Story = {
  name: "Claude Code · план изменений",
  parameters: { scenario: "all_agents", route: "/agents/claude" },
  play: async ({ canvas, canvasElement }) => {
    const plan = await openClaudeCodePlan(canvas, within(canvasElement.ownerDocument.body));
    await expect(plan.getByText("coreinfra_media", { selector: "code" })).toBeVisible();
    const details = plan.getByRole("button", { name: /Ещё .*шаг/ });
    await expect(details).toHaveAttribute("aria-expanded", "false");
  },
};

export const AgentSettingsApplied: Story = {
  name: "Claude Code · настройки применены",
  parameters: { scenario: "all_agents", route: "/agents/claude" },
  play: async ({ canvas, canvasElement }) => {
    const page = within(canvasElement.ownerDocument.body);
    const plan = await openClaudeCodePlan(canvas, page);
    await userEvent.click(plan.getByRole("button", { name: "Применить" }));
    const success = within(await page.findByRole("dialog", { name: "Настройки применены" }));
    await expect(success.getByRole("button", { name: "Готово" })).toBeVisible();
    await expect(success.getByRole("button", { name: "Запустить агента" })).toBeEnabled();
  },
};
export const SubscriptionKey: Story = {
  name: "Подключение · выбор ключа для API и подписки",
  parameters: { route: "/agents/claude-desktop" },
  play: async ({ canvas, canvasElement, step }) => {
    const page = within(canvasElement.ownerDocument.body);
    await step("Выбрать ключ для API", async () => {
      await userEvent.click(await canvas.findByRole("button", { name: "Изменить токен" }));
      await userEvent.click(await page.findByRole("menuitem", { name: /Команда · демо/ }));
      await expect(canvas.getByRole("button", { name: "Применить" })).toBeEnabled();
    });
    await step("Сохранить выбранный ключ при переходе на подписку", async () => {
      await userEvent.click(canvas.getByRole("radio", { name: "Своя подписка Claude" }));
      const prompt = await page.findByRole("dialog", { name: "Включить системный прокси?" });
      await userEvent.click(within(prompt).getByRole("button", { name: "Пропустить" }));
      await waitFor(() => expect(page.queryByRole("dialog")).not.toBeInTheDocument());
      await expect(canvas.getByText(/Будет использован токен «Команда · демо»/)).toBeVisible();
      await userEvent.click(canvas.getByRole("button", { name: "Изменить токен" }));
      await expect(await page.findByRole("menuitem", { name: /Команда · демо/ })).toHaveAttribute("aria-current", "true");
      await userEvent.keyboard("{Escape}");
    });
    await step("Применить подключение", async () => {
      await userEvent.click(canvas.getByRole("button", { name: "Применить" }));
      const plan = await page.findByRole("dialog", { name: "План изменений: Claude Desktop" });
      await userEvent.click(within(plan).getByRole("button", { name: "Применить" }));
      const success = await page.findByRole("dialog", { name: "Настройки применены" });
      await userEvent.click(within(success).getByRole("button", { name: "Готово" }));
      await waitFor(() => expect(page.queryByRole("dialog")).not.toBeInTheDocument());
      await expect(canvas.getByRole("radio", { name: "Своя подписка Claude" })).toBeChecked();
      await expect(canvas.getByText(/Будет использован токен «Команда · демо»/)).toBeVisible();
      await expect(canvas.getByRole("button", { name: "Применить" })).toBeDisabled();
    });
  },
};
export const ReadyPicker: Story = {
  ...Picker,
  name: "Выбор · оба скачаны",
  parameters: { scenario: "ready", onboarding: true },
};
export const MixedReadyPicker: Story = {
  ...Picker,
  name: "Выбор · скачано и доступно",
  parameters: { scenario: "mixed_ready", onboarding: true },
};
export const MixedProgressPicker: Story = {
  ...Picker,
  name: "Выбор · скачано и скачивается",
  parameters: { scenario: "mixed_progress", onboarding: true },
};
export const MixedErrorPicker: Story = {
  ...Picker,
  name: "Выбор · ошибка и скачано",
  parameters: { scenario: "mixed_error", onboarding: true },
};
export const MissingPicker: Story = {
  ...Picker,
  name: "Выбор · приложения не установлены",
  parameters: { scenario: "missing", onboarding: true },
};
export const CurrentPicker: Story = {
  ...Picker,
  name: "Выбор · актуальные версии",
  parameters: { scenario: "current", onboarding: true },
};
export const SetupSelection: Story = {
  name: "Настройка · выбор агента",
  parameters: { scenario: "all_agents", onboarding: true },
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(
      await canvas.findByRole("button", { name: "Настроить агента" }),
    );
    await expectVisible(
      within(canvasElement.ownerDocument.body).findByRole("dialog", {
        name: "Настройка агентов",
      }),
    );
  },
};
export const SetupConfiguration: Story = {
  name: "Панель управления · статус → подключение",
  parameters: { scenario: "all_agents" },
  play: async ({ canvas, canvasElement }) => {
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(await canvas.findByRole("button", { name: "Панель управления" }));
    const management = await page.findByRole("dialog", { name: "Управление агентами" });
    await userEvent.click(within(management).getByRole("button", { name: "Настройки подключения Claude Desktop: CoreInfra AI Hub" }));
    await expectVisible(within(management).findByRole("radio", { name: "API-доступ" }));
    await expect(within(management).getByRole("button", { name: "Применить" })).toBeDisabled();
  },
};
export const TwoDownloads: Story = {
  name: "Две независимые загрузки",
  parameters: { onboarding: true },
  play: async ({ canvas, canvasElement }) => {
    const page = within(canvasElement.ownerDocument.body);
    let dialog = await openPicker(canvas, page);
    await userEvent.click(
      within(dialog).getAllByRole("button", { name: "Обновить" })[0],
    );
    await userEvent.click(await page.findByRole("button", { name: "Скачать обновление" }));
    await userEvent.click(await page.findByRole("button", { name: "Назад" }));
    dialog = await page.findByRole("dialog", { name: "Установите агентов" });
    await userEvent.click(
      within(within(dialog).getByRole("article", { name: "ChatGPT" })).getByRole("button", { name: "Обновить" }),
    );
    await userEvent.click(await page.findByRole("button", { name: "Скачать обновление" }));
    await userEvent.click(await page.findByRole("button", { name: "Назад" }));
    dialog = await page.findByRole("dialog", { name: "Установите агентов" });
    await expect(within(dialog).getAllByRole("progressbar")).toHaveLength(2);
  },
};
export const Ready: Story = {
  name: "Скачано → установка",
  parameters: { scenario: "ready", route: "/agents/claude-desktop" },
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(
      await canvas.findByRole("button", { name: "Обновить" }),
    );
    const page = within(canvasElement.ownerDocument.body);
    const dialog = await page.findByRole("dialog");
    await waitFor(() =>
      expect(
        within(dialog).getByRole("button", { name: "Установить" }),
      ).toBeEnabled(),
    );
  },
};
export const InstallWithConfirmation: Story = {
  name: "Подтверждение → установка → готово",
  parameters: { scenario: "confirmation", route: "/agents/claude-desktop" },
  play: async ({ canvas, canvasElement, step }) => {
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Обновить" }),
    );
    const dialog = await page.findByRole("dialog");
    const install = within(dialog).getByRole("button", { name: "Установить" });
    await expect(install).toBeDisabled();
    await step("Подтвердить замену приложения", async () => {
      await userEvent.click(await within(dialog).findByRole("checkbox"));
      await waitFor(() => expect(install).toBeEnabled());
    });
    await userEvent.click(install);
    await expectVisible(
      within(dialog).findByRole(
        "button",
        { name: "Готово" },
        { timeout: 5000 },
      ),
    );
  },
};
export const InstallFailure: Story = {
  name: "Ошибка установки и повтор",
  parameters: { scenario: "install_error", route: "/agents/claude-desktop" },
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(
      await canvas.findByRole("button", { name: "Обновить" }),
    );
    await expectVisible(
      within(canvasElement.ownerDocument.body).findByRole("button", {
        name: "Повторить установку",
      }),
    );
  },
};
export const Setup: Story = {
  name: "Выбор → настройка → план → завершение",
  parameters: { scenario: "onboarding_merge" },
  play: async ({ canvas, canvasElement, step }) => {
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Настроить агента" }),
    );
    await step("Выбрать агента", async () => {
      await userEvent.click(
        await page.findByRole("radio", { name: "Claude Desktop" }),
      );
      const dialog = page.getByRole("dialog", { name: "Настройка агентов" });
      await userEvent.click(
        within(dialog).getByRole("button", { name: "Настроить агента" }),
      );
    });
    await step("Просмотреть план", async () => {
      await userEvent.click(
        await page.findByRole("button", { name: "Завершить настройку" }),
      );
      await expectVisible(
        page.findByRole("dialog", {
          name: "План изменений: Claude Desktop",
        }),
      );
    });
    await step("Применить демонстрационный план", async () => {
      const plan = page.getByRole("dialog", {
        name: "План изменений: Claude Desktop",
      });
      await userEvent.click(
        within(plan).getByRole("button", { name: "Завершить настройку" }),
      );
      await expectVisible(
        page.findByRole("dialog", { name: "Настройки применены" }),
      );
    });
    await step("Перейти к рабочему обзору", async () => {
      await userEvent.click(page.getByRole("button", { name: "Готово" }));
      await expectVisible(canvas.findByRole("heading", { name: "Агент готов!" }));
      await expect(canvas.queryByRole("heading", { name: "Начало работы" })).not.toBeInTheDocument();
      const dashboard = canvas.getByRole("region", { name: "Хаб и агенты" });
      await expect(within(dashboard).getByRole("button", { name: "Панель управления" })).toBeVisible();
      await expect(within(dashboard).queryByRole("button", { name: /Установить/ })).not.toBeInTheDocument();
    });
  },
};
export const Backups: Story = {
  name: "Резервные копии",
  parameters: { route: "/agents/claude-desktop" },
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(
      await canvas.findByRole("button", { name: "Резервные копии настроек" }),
    );
    const page = within(canvasElement.ownerDocument.body);
    await expectVisible(
      page.findByRole("dialog", {
        name: "Резервные копии настроек Claude Desktop",
      }),
    );
  },
};
export const EmptyBackups: Story = {
  ...Backups,
  name: "Пустые резервные копии",
  parameters: { scenario: "empty_backups", route: "/agents/claude-desktop" },
};
export const FinishDownload: Story = {
  name: "Прогресс → готово к установке",
  parameters: { route: "/agents/claude-desktop" },
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(
      await canvas.findByRole("button", { name: "Обновить" }),
    );
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(await page.findByRole("button", { name: "Скачать обновление" }));
    await page.findByRole("button", { name: "Отменить" });
    finishDownloads();
    const dialog = await page.findByRole("dialog");
    await expectVisible(
      within(dialog).findByRole("button", { name: "Установить" }),
    );
  },
};

export const AgentUpdate: Story = {
  name: "Обновить → скачать → установить → новая версия",
  parameters: { route: "/agents/claude-desktop" },
  play: async ({ canvas, canvasElement, step }) => {
    const page = within(canvasElement.ownerDocument.body);
    await step("Открыть панель обновления", async () => {
      await userEvent.click(await canvas.findByRole("button", { name: "Обновить" }));
      await expectVisible(page.findByRole("dialog", { name: "Установка агента" }));
    });
    const dialog = page.getByRole("dialog", { name: "Установка агента" });
    await step("Скачать обновление", async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Скачать обновление" }));
      await within(dialog).findByRole("progressbar", { name: "Скачивание Claude Desktop" });
      finishDownloads();
      await waitFor(() => expect(within(dialog).getByRole("button", { name: "Установить" })).toBeEnabled());
    });
    await step("Установить и обновить номер версии", async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Установить" }));
      await userEvent.click(await within(dialog).findByRole("button", { name: "Готово" }, { timeout: 5000 }));
      await waitFor(() => expect(page.queryByRole("dialog")).not.toBeInTheDocument());
      await expect(canvas.getByText("v1.0.124")).toBeVisible();
      await expect(canvas.queryByRole("button", { name: "Обновить" })).not.toBeInTheDocument();
      await expect(canvas.queryByText("Версия актуальна")).not.toBeInTheDocument();
    });
  },
};
