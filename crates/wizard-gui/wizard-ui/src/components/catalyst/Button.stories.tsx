import type { Meta, StoryObj } from "@storybook/react-vite";
import { ArrowDownTrayIcon, XMarkIcon } from "@heroicons/react/20/solid";
import { expect, fn, userEvent } from "storybook/test";
import { Button } from "./button";

const meta = {
  title: "Catalyst/Кнопки",
  component: Button,
  tags: ["autodocs"],
  args: { children: "Скачать обновление", onClick: fn() },
  argTypes: {
    size: { control: "inline-radio", options: ["sm", "md", "lg"] },
    disabled: { control: "boolean" },
  },
} satisfies Meta<typeof Button>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Primary: Story = {
  name: "Основная",
  play: async ({ canvas, args }) => {
    await userEvent.click(canvas.getByRole("button"));
    await expect(args.onClick).toHaveBeenCalledOnce();
  },
};
export const Secondary: Story = { name: "Вторичная", args: { outline: true } };
export const Quiet: Story = {
  name: "Текстовая",
  args: { plain: true, children: "Назад" },
};
export const Disabled: Story = {
  name: "Недоступна",
  args: { disabled: true },
  play: async ({ canvas }) => {
    await expect(canvas.getByRole("button")).toBeDisabled();
  },
};
export const Busy: Story = {
  name: "Выполнение",
  args: { disabled: true, "aria-busy": true, children: "Устанавливаем…" },
};
export const WithIcon: Story = {
  name: "С иконкой",
  args: {
    outline: true,
    children: (
      <>
        <ArrowDownTrayIcon />
        Скачать обновление
      </>
    ),
  },
};
export const IconOnly: Story = {
  name: "Только иконка",
  args: {
    plain: true,
    size: "sm",
    "aria-label": "Закрыть",
    children: <XMarkIcon />,
  },
};
export const Sizes: Story = {
  name: "Размеры",
  render: () => (
    <div className="flex flex-wrap items-center gap-4">
      <Button size="sm">Компактная</Button>
      <Button>Основная</Button>
      <Button size="lg">Крупная</Button>
    </div>
  ),
};
export const LongLabel: Story = {
  name: "Длинная подпись",
  args: { children: "Подтвердить изменения и завершить настройку агента" },
  decorators: [
    (Story) => (
      <div className="max-w-xs">
        <Story />
      </div>
    ),
  ],
};
