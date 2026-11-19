import { useState } from "react";
import App from "../App";
import { SubscriptionTokenPreviewContext } from "../agents/connection_preview";
import { getTheme, setTheme } from "../settings/theme";
import { scenario, scenarios, finishDownloads, setDownloadSpeed, showOverviewTransition } from "./mocks";
import "../styles.css";
import "./preview.css";

function PreviewToolbar() {
  const [fast, setFast] = useState(false);
  const [dark, setDark] = useState(getTheme() === "dark");
  return <details className="preview-controls">
    <summary>UI-превью · Моки</summary>
    <div className="preview-controls-body">
      <p>Демонстрационные версии. Файлы не скачиваются, приложения не изменяются.</p>
      <label htmlFor="preview-scenario">Сценарий</label>
      <select id="preview-scenario" value={scenario} onChange={(event) => {
        const url = new URL(window.location.href);
        url.searchParams.set("scenario", event.target.value);
        window.location.href = url.href;
      }}>{Object.entries(scenarios).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select>
      <div className="preview-controls-actions">
        {scenario === "onboarding_merge" && <button onClick={(event) => {
          event.currentTarget.closest("details")?.removeAttribute("open");
          showOverviewTransition();
        }}>Показать переход</button>}
        <button onClick={() => { setFast(!fast); setDownloadSpeed(fast ? 1 : 6); }}>Скорость: {fast ? "×6" : "×1"}</button>
        <button onClick={finishDownloads}>Завершить загрузки</button>
        <button onClick={() => { setTheme(dark ? "light" : "dark"); setDark(!dark); }}>{dark ? "Светлая тема" : "Тёмная тема"}</button>
        <button onClick={() => window.location.reload()}>Сбросить</button>
      </div>
      <p>Закройте панель перед проверкой. {scenario === "startup" ? "Сброс повторяет анимацию первого запуска. Затем открывается весь интерфейс без входа в аккаунт."
        : scenario === "onboarding_merge" ? "«Показать переход» сворачивает карточку установки в кнопку «Панель управления». Повторить — через «Сбросить». Полный сценарий также доступен через настройку Claude Desktop."
        : scenario === "wizard_update" ? "Кнопка обновления в левой панели открывает окно. Скачивание и установка имитируются; приложение не меняется."
        : scenario === "balance_empty" ? "«Пополнить» имитирует пополнение: через 0,6 секунды баланс станет 1 500 ₽. Настоящих платежей нет."
        : scenario === "checking" ? "Проверка идёт в фоне: через 3 секунды появятся версии для скачивания."
          : scenario === "downloads" ? "Загрузки замедлены, чтобы рассмотреть виджеты в левой панели." : "Загрузка длится около 36 секунд. Установка новых приложений доступна в панели управления."}</p>
    </div>
  </details>;
}

export default function PreviewApp() {
  return <SubscriptionTokenPreviewContext.Provider value={true}>
    <App />
    <PreviewToolbar />
  </SubscriptionTokenPreviewContext.Provider>;
}
