import type { Meta, StoryObj } from "@storybook/react-vite";
import { PageHeader, Panel } from "../components/ui/Layout";
import { Heading, Subheading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";
import { Button } from "../components/catalyst/button";
import { StateNotice } from "../components/ui/StateNotice";

const meta = {
  title: "Начало/Дизайн-система",
  parameters: { layout: "padded" },
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;

export const Concept: Story = {
  name: "Концепция и правила",
  render: () => (
    <div className="ui-page ui-stack">
      <PageHeader
        title="Wizard · спокойный рабочий интерфейс"
        description="Catalyst задаёт основу. Общие компоненты задают ритм, поведение и состояния приложения."
      />
      <div className="grid gap-4 md:grid-cols-2">
        <Panel title="Иерархия">
          <Text>
            Один главный заголовок на странице. Разделы отделяются
            пространством, связанные настройки объединяются в панели. Цвет
            обозначает состояние, а не украшает каждый блок.
          </Text>
        </Panel>
        <Panel title="Воздух и выравнивание">
          <Text>
            Шаг 4 px. Отступы панелей 24 / 20 px, между карточками 16 px.
            Карточки в строке одинаковой высоты; действия закреплены внизу.
            Длинный текст переносится, не сдвигая соседние кнопки.
          </Text>
        </Panel>
        <Panel title="Состояния">
          <Text>
            Загрузка, пустое состояние, готовность, ошибка и повтор — части
            каждого компонента. Проверка обновлений автоматическая. Закрытие
            модалки сохраняет загрузку, отмена останавливает её.
          </Text>
        </Panel>
        <Panel title="Движение">
          <Text>
            Сохраняем стартовый логотип, рисунки обзора и прогресс. Переходы
            экранов и диалогов короткие. Повторный рендер прогресса не
            перезапускает входную анимацию. Учитываем уменьшение движения.
          </Text>
        </Panel>
      </div>
      <Panel title="Как проверять">
        <Text>
          В верхней панели переключайте светлую и тёмную темы, движение и
          размеры: 360, 640, 800, 1100 и 1440 px. Каталог идёт от основ и
          Catalyst к блокам и настоящим экранам с локальными моками.
        </Text>
        <div className="mt-4">
          <StateNotice>
            Все сценарии изолированы от Tauri: скачивание, настройки, аккаунт и
            установка в Storybook изменяют только демонстрационные данные.
          </StateNotice>
        </div>
      </Panel>
    </div>
  ),
};

export const Typography: Story = {
  name: "Типографика",
  render: () => (
    <div className="ui-page ui-stack">
      <PageHeader
        title="Типографика"
        description="Inter Variable. Размер заголовка определяется уровнем компонента, а не селектором родительской страницы."
      />
      <Panel>
        <div className="ui-stack">
          <Heading>Страница · 22 / 32 · 600</Heading>
          <Heading level={2}>Раздел и диалог · 16 / 24 · 600</Heading>
          <Heading level={3}>Карточка · 14 / 24 · 600</Heading>
          <Text>
            Основной текст · 14 / 24. Достаточно воздуха для длинных описаний и
            русского текста.
          </Text>
          <Subheading>Подзаголовок и подпись действия</Subheading>
          <span className="text-xs/5 text-zinc-500">Метаданные · 12 / 20</span>
          <span className="font-mono text-xs/5">
            248 / 248 МБ · технические данные
          </span>
        </div>
      </Panel>
    </div>
  ),
};

export const Palette: Story = {
  name: "Семантические цвета",
  render: () => (
    <div className="ui-page ui-stack">
      <PageHeader
        title="Цвета"
        description="Компоненты используют назначение цвета. Светлая и тёмная темы меняют значения токенов."
      />
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-3">
        {[
          "canvas",
          "surface",
          "surface-muted",
          "ink",
          "ink-muted",
          "ink-subtle",
          "border",
          "focus",
          "success",
          "warning",
          "danger",
          "progress",
        ].map((name) => (
          <Panel key={name}>
            <div
              className="mb-3 h-16 rounded-lg border border-zinc-500/20"
              style={{ background: `var(--ui-${name})` }}
            />
            <Text>--ui-{name}</Text>
          </Panel>
        ))}
      </div>
    </div>
  ),
};

export const Spacing: Story = {
  name: "Ритм и геометрия",
  render: () => (
    <div className="ui-page ui-stack">
      <PageHeader title="Ритм и геометрия" />
      <Panel title="Шкала отступов">
        <div className="grid gap-4">
          {[1, 2, 3, 4, 5, 6, 8, 10].map((step) => (
            <div key={step} className="flex items-center gap-4">
              <span className="w-16 text-xs tabular-nums">{step * 4} px</span>
              <span
                className="h-6 rounded-sm bg-zinc-400"
                style={{ width: `var(--ui-space-${step})` }}
              />
            </div>
          ))}
        </div>
      </Panel>
      <Panel title="Размеры действий">
        <div className="flex flex-wrap items-center gap-4">
          <Button size="sm">Компактная</Button>
          <Button>Основная</Button>
          <Button size="lg">Крупная</Button>
        </div>
        <Text className="mt-4">
          Кнопки 32 / 36 / 44 px. На узком экране основная кнопка — 40 px.
          Область нажатия Catalyst расширяет маленькие цели до 44 px.
        </Text>
      </Panel>
    </div>
  ),
};
