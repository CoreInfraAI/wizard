import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import App from "../App";
import { showOverviewTransition } from "./mocks";
import "./preview.css";

function TransitionStory() {
  const [played, setPlayed] = useState(false);
  return <>
    <App />
    <details className="preview-controls" data-story="overview-transition">
      <summary>История · Переход в панель управления</summary>
      <div className="preview-controls-body">
        <p>После настройки первого агента карточка установки сворачивается в кнопку «Панель управления». Новые приложения устанавливаются внутри панели.</p>
        <div className="preview-controls-actions">
          <button disabled={played} onClick={(event) => {
            event.currentTarget.closest("details")?.removeAttribute("open");
            setPlayed(true);
            showOverviewTransition();
          }}>{played ? "Переход показан" : "Показать переход"}</button>
          <button onClick={() => window.location.reload()}>Сбросить</button>
        </div>
        <p>Все действия выполняются на моках. «Сбросить» возвращает три карточки онбординга.</p>
      </div>
    </details>
  </>;
}

const meta = {
  id: "overview-transition",
  title: "Сценарии/Переход в панель управления",
  component: TransitionStory,
  parameters: { layout: "fullscreen", scenario: "onboarding_merge", route: "/overview" },
  tags: ["!autodocs"],
} satisfies Meta<typeof TransitionStory>;
export default meta;
type Story = StoryObj<typeof meta>;

export const InstallationIntoControlPanel: Story = {
  name: "Установка → Панель управления",
};
