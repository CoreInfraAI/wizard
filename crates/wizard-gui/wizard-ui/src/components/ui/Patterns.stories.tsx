import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { ArrowDownTrayIcon } from "@heroicons/react/20/solid";
import { ActionBar, PageHeader, Panel, ViewTransition } from "./Layout";
import { EmptyState, StateNotice } from "./StateNotice";
import { Identity } from "./Identity";
import { Button } from "../catalyst/button";
import { Text } from "../catalyst/text";
import { ClaudeIcon } from "../BrandIcons";
import { AppLogo } from "../AppLogo";
import { StartupScreen } from "../StartupScreen";

const meta = { title: "Блоки/Композиция", tags: ["autodocs"] } satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;
export const PageTitle: Story = {
  name: "Заголовок страницы",
  render: () => (
    <PageHeader
      title="Claude Desktop"
      description="Подключение и установленные версии агента."
      actions={<Button outline>Резервные копии настроек</Button>}
    />
  ),
};
export const Section: Story = {
  name: "Панель",
  render: () => (
    <Panel title="Приложение" description="Параметры работы Wizard.">
      <Text>Связанные настройки объединены в один блок.</Text>
      <ActionBar divider>
        <Button>Применить</Button>
      </ActionBar>
    </Panel>
  ),
};
export const Actions: Story = {
  name: "Действия",
  render: () => (
    <Panel>
      <Text>
        Основное действие справа, навигация слева. На узком экране действия
        переносятся, сохраняя порядок.
      </Text>
      <ActionBar divider leading={<Button plain>Назад</Button>}>
        <Button outline>Отменить</Button>
        <Button>Установить</Button>
      </ActionBar>
    </Panel>
  ),
};
export const Notices: Story = {
  name: "Состояния",
  render: () => (
    <div className="ui-stack max-w-2xl">
      <StateNotice tone="loading">
        Проверяем установленные приложения…
      </StateNotice>
      <StateNotice>Изменения не требуются.</StateNotice>
      <StateNotice tone="success">Настройки сохранены.</StateNotice>
      <StateNotice tone="warning">
        Соединение потеряно. Восстанавливаем загрузку.
      </StateNotice>
      <StateNotice tone="error" actions={<Button outline>Повторить</Button>}>
        Не удалось прочитать конфигурацию агента.
      </StateNotice>
    </div>
  ),
};
export const Empty: Story = {
  name: "Пустое состояние",
  render: () => (
    <Panel>
      <EmptyState
        icon={ArrowDownTrayIcon}
        title="Агенты не найдены"
        action={<Button>Установить агента</Button>}
      >
        Установите приложение, чтобы настроить подключение.
      </EmptyState>
    </Panel>
  ),
};
export const AgentIdentity: Story = {
  name: "Название и логотип",
  render: () => (
    <Identity
      icon={<ClaudeIcon />}
      title="Claude Desktop"
      description="Версия 1.0.124"
    />
  ),
};
export const Logo: Story = {
  name: "Логотип",
  render: () => <AppLogo className="w-64" />,
};
function TransitionExample() {
  const [view, setView] = useState("выбор");
  return (
    <Panel>
      <div className="mb-6 flex gap-3">
        <Button outline onClick={() => setView("выбор")}>
          Выбор
        </Button>
        <Button outline onClick={() => setView("настройка")}>
          Настройка
        </Button>
      </div>
      <ViewTransition view={view}>
        <Identity
          icon={<ClaudeIcon />}
          title={view === "выбор" ? "Выберите агента" : "Настройте подключение"}
        />
        <Text className="mt-4">
          Переход на новое содержимое без анимации каждого обновления прогресса.
        </Text>
      </ViewTransition>
    </Panel>
  );
}
export const Transitions: Story = {
  name: "Переходы React",
  render: () => <TransitionExample />,
};
function StartupExample() {
  const [complete, setComplete] = useState(false);
  return complete ? (
    <div className="p-6">
      <Panel title="Wizard готов">
        <Button outline onClick={() => setComplete(false)}>
          Повторить анимацию
        </Button>
      </Panel>
    </div>
  ) : (
    <StartupScreen onComplete={() => setComplete(true)} />
  );
}
export const Startup: Story = {
  name: "Стартовая анимация",
  parameters: { layout: "fullscreen" },
  render: () => <StartupExample />,
};
