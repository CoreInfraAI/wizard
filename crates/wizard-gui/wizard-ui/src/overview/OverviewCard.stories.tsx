import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { Button } from "../components/catalyst/button";
import { OverviewArtwork } from "./OverviewArtwork";
import { OverviewCard, OverviewCardContent } from "./OverviewCard";
import "./Overview.css";

const meta = {
  title: "Блоки/Шаг обзора",
  component: OverviewCard,
  tags: ["autodocs"],
  args: { kind: "install", disabled: false },
  decorators: [
    (Story) => (
      <div className="max-w-sm">
        <Story />
      </div>
    ),
  ],
  render: (args) => (
    <OverviewCard {...args}>
      <OverviewArtwork
        kind={args.kind}
        disabled={args.disabled}
        label={
          args.kind === "account" ? "Подключить аккаунт" : "Открыть установку"
        }
        onClick={fn()}
      />
      <OverviewCardContent
        title={
          args.kind === "account"
            ? "Подключите свой аккаунт"
            : "Установите агентов"
        }
        description={
          args.kind === "account"
            ? "Войдите в CoreInfra Hub, чтобы начать работу."
            : "Wizard скачает и установит Claude или ChatGPT приложения!"
        }
        actions={
          <Button outline disabled={args.disabled}>
            {args.kind === "account" ? "Войти" : "Установить агента"}
          </Button>
        }
      />
    </OverviewCard>
  ),
} satisfies Meta<typeof OverviewCard>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Available: Story = {
  name: "Доступный шаг",
  args: { children: null },
};
export const Locked: Story = {
  name: "Недоступный шаг",
  args: { children: null, disabled: true },
};
export const Account: Story = {
  name: "Анимация аккаунта",
  args: { children: null, kind: "account" },
};
