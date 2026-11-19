import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { TransferCard } from "./TransferCard";
import { ClaudeIcon, OpenAiIcon } from "../BrandIcons";
import { Button } from "../catalyst/button";

const meta = {
  title: "Блоки/Карточка установки",
  component: TransferCard,
  tags: ["autodocs"],
  args: {
    name: "Claude Desktop",
    brand: "claude",
    icon: <ClaudeIcon />,
    version: "Версия 1.0.121",
    status: "Доступна версия 1.0.124",
    action: <Button outline>Обновить</Button>,
  },
  argTypes: {
    progress: { control: { type: "range", min: 0, max: 100, step: 1 } },
    variant: { control: "inline-radio", options: ["card", "compact"] },
    tone: { control: "select", options: ["default", "success", "error"] },
  },
  decorators: [
    (Story, context) => (
      <div className={context.parameters.wide ? "max-w-3xl" : "max-w-sm"}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof TransferCard>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Available: Story = { name: "Обновление доступно" };
export const Missing: Story = {
  name: "Первая установка",
  args: {
    version: undefined,
    status: "Приложение Anthropic с Cowork и Claude Code.",
    action: <Button outline>Скачать</Button>,
  },
};
export const Checking: Story = {
  name: "Проверка",
  args: { status: "Проверяем версию…", action: undefined },
};
export const Current: Story = {
  name: "Актуальная версия",
  args: { status: "", action: undefined },
};
export const Downloading: Story = {
  name: "Загрузка",
  args: {
    version: undefined,
    status: "Скачивается · 38%",
    detail: "94 / 248 МБ",
    downloading: true,
    progress: 38,
    action: <Button outline>Отменить</Button>,
  },
};
export const UnknownSize: Story = {
  name: "Неизвестный размер",
  args: {
    version: undefined,
    status: "Скачивается…",
    detail: "42 / … МБ",
    downloading: true,
    progress: undefined,
    action: <Button outline>Отменить</Button>,
  },
};
export const Reconnecting: Story = {
  name: "Переподключение",
  args: { ...Downloading.args, notice: "Восстанавливаем соединение…" },
};
export const Ready: Story = {
  name: "Готово к установке",
  args: {
    version: undefined,
    status: "Версия 1.0.124 — готова к установке",
    filled: true,
    action: (
      <div className="ui-transfer-action-group">
        <Button plain>Отменить</Button>
        <Button>Установить</Button>
      </div>
    ),
  },
};
export const Installing: Story = {
  name: "Установка",
  args: {
    version: undefined,
    status: "Устанавливается…",
    filled: true,
    action: <Button disabled>Устанавливаем…</Button>,
  },
};
export const Installed: Story = {
  name: "Готово",
  args: {
    version: undefined,
    status: "Установлена версия 1.0.124",
    filled: true,
    tone: "success",
    action: <Button>Готово</Button>,
  },
};
export const Failed: Story = {
  name: "Ошибка",
  args: {
    status: "Не удалось скачать приложение",
    tone: "error",
    action: <Button outline>Повторить</Button>,
  },
};
export const Sidebar: Story = {
  name: "Виджет в панели",
  args: {
    variant: "compact",
    status: "94 / 248 МБ",
    downloading: true,
    progress: 38,
    onOpen: fn(),
    action: undefined,
  },
  decorators: [
    (Story) => (
      <div className="w-56">
        <Story />
      </div>
    ),
  ],
};
export const SidebarReady: Story = {
  name: "Виджет после загрузки",
  args: {
    variant: "compact",
    status: "Готово к установке",
    filled: true,
    onOpen: fn(),
    action: undefined,
  },
  decorators: [
    (Story) => (
      <div className="w-56">
        <Story />
      </div>
    ),
  ],
};
export const AlignedPair: Story = {
  name: "Выравнивание и длинный текст",
  parameters: { wide: true },
  render: () => (
    <div className="grid items-stretch gap-4 sm:grid-cols-2">
      <TransferCard
        name="Claude Desktop"
        brand="claude"
        icon={<ClaudeIcon />}
        version="Версия 1.0.121"
        status="Доступна версия 1.0.124"
        action={<Button outline>Скачать обновление</Button>}
      />
      <TransferCard
        name="ChatGPT"
        brand="chatgpt"
        icon={<OpenAiIcon />}
        version="Версия 1.2026.310"
        status="Доступна новая версия приложения 1.2026.321"
        action={<Button outline>Скачать обновление</Button>}
      />
    </div>
  ),
};
