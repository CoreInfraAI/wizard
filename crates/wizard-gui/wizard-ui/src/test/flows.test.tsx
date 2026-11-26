import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks } from "@tauri-apps/api/mocks";
import App from "../App";
import * as agentCommands from "../agents/agents_state";
import { SubscriptionTokenPreviewContext } from "../agents/connection_preview";
import { systemProxyBannerDismissedKey } from "../overview/SystemProxyBanner";
import { createMockEnvironment, type MockOptions, type Scenario } from "../preview/mocks";

let environment: ReturnType<typeof createMockEnvironment>;
function launch(
  scenario: Scenario = "update",
  route = "/agents/claude-desktop",
  prepare?: (environment: ReturnType<typeof createMockEnvironment>) => void,
  options?: MockOptions,
) {
  environment = createMockEnvironment(scenario, options);
  environment.start();
  prepare?.(environment);
  window.location.hash = route;
  return render(
    <SubscriptionTokenPreviewContext.Provider value={true}>
      <App />
    </SubscriptionTokenPreviewContext.Provider>,
  );
}
async function openInstallationPicker(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "Установить агента" }));
  return screen.findByRole("dialog", { name: "Установите агентов" });
}
async function finishSettings(user: ReturnType<typeof userEvent.setup>) {
  const success = await screen.findByRole("dialog", { name: "Настройки применены" });
  await user.click(within(success).getByRole("button", { name: "Готово" }));
}
beforeEach(() => {
  localStorage.setItem("coreinfra.startup.seen", "true");
  localStorage.removeItem(systemProxyBannerDismissedKey);
});
afterEach(() => {
  cleanup();
  environment?.dispose();
  clearMocks();
});

describe("overview system proxy banner", () => {
  const name = "Для доступа к OpenAI и Anthropic из браузера включите системный прокси.";

  it("remembers Skip after reopening Wizard and leaves the proxy off", async () => {
    const user = userEvent.setup();
    const app = launch("unconfigured", "/overview");
    const banner = await screen.findByRole("region", { name });
    await user.click(within(banner).getByRole("button", { name: "Пропустить" }));
    expect(banner).not.toBeInTheDocument();

    app.unmount();
    render(<App />);
    await screen.findByRole("heading", { name: "Начало работы" });
    expect(screen.queryByRole("region", { name })).not.toBeInTheDocument();
    expect(environment.systemProxy.value.status).toBe("off");
    expect(environment.calls).not.toContain("set_system_proxy");
  });

  it("reports a conflicting proxy and lets the user retry the normal Settings action", async () => {
    const user = userEvent.setup();
    launch("unconfigured", "/overview");
    const banner = await screen.findByRole("region", { name });
    const enable = within(banner).getByRole("button", { name: "Включить" });
    const proxy = environment.systemProxy;
    expect(environment.calls).not.toContain("set_system_proxy");

    // Another proxy appeared after the last snapshot reached the UI.
    proxy.value = { ...proxy.value, conflict: true };
    await user.click(enable);
    expect(await within(banner).findByRole("alert")).toHaveTextContent("Обнаружены другие настройки прокси");
    expect(proxy.value.status).toBe("off");
    await act(async () => proxy.publish({ ...proxy.value, conflict: false }));
    await user.click(enable);
    expect(enable).toBeDisabled();
    expect(within(banner).getByRole("button", { name: "Пропустить" })).toBeDisabled();
    await waitFor(() => expect(banner).not.toBeInTheDocument());
    expect(proxy.value.status).toBe("on");

    await user.click(screen.getByRole("link", { name: "Настройки" }));
    const toggle = await screen.findByRole("switch", { name: "Использовать прокси Wizard" });
    await waitFor(() => expect(toggle).toBeChecked());
  });

  it("follows sign-in and live proxy state before making the offer", async () => {
    launch("offline", "/overview");
    await screen.findByRole("button", { name: "Войти" });
    expect(screen.queryByRole("region", { name })).not.toBeInTheDocument();

    const { account, systemProxy } = environment;
    await act(async () => {
      systemProxy.publish({ ...systemProxy.value, conflict: true });
      account.publish({ ...account.value, signed_in: true });
    });
    expect(screen.queryByRole("region", { name })).not.toBeInTheDocument();
    await act(async () => systemProxy.publish({ ...systemProxy.value, conflict: false }));
    const banner = await screen.findByRole("region", { name });
    await act(async () => systemProxy.publish({ ...systemProxy.value, status: "on" }));
    expect(banner).not.toBeInTheDocument();
    expect(environment.calls).not.toContain("set_system_proxy");
  });
});

it("keeps media local to the draft until confirmation and saves the preference only for the selected agent", async () => {
  const user = userEvent.setup();
  launch("all_agents", "/agents/codex");
  const name = "Подключить coreinfra-media для видео, озвучки и распознавания речи";
  const toggle = await screen.findByRole("switch", { name });
  expect(toggle).not.toBeChecked();
  await user.click(toggle);
  await user.click(screen.getByRole("button", { name: "Применить" }));
  let plan = await screen.findByRole("dialog", { name: "План изменений: ChatGPT / Codex" });
  expect(within(plan).getByText("coreinfra_media", { selector: "code" })).toBeVisible();
  await user.click(within(plan).getByRole("button", { name: "Отмена" }));
  await user.click(screen.getByRole("link", { name: "Claude Code" }));
  const claude = await screen.findByRole("switch", { name });
  expect(claude).not.toBeChecked();
  await user.click(claude);
  await user.click(screen.getByRole("button", { name: "Применить" }));
  plan = await screen.findByRole("dialog", { name: "План изменений: Claude Code" });
  expect(environment.calls).not.toContain("set_media_connection");
  await user.click(within(plan).getByRole("button", { name: "Применить" }));
  const success = await screen.findByRole("dialog", { name: "Настройки применены" });
  expect(environment.calls.filter(command => command === "set_media_connection")).toHaveLength(1);
  expect(environment.calls).not.toContain("apply_agent_plan");
  expect(environment.calls).not.toContain("launch_agent");
  await user.click(within(success).getByRole("button", { name: "Запустить агента" }));
  expect(environment.calls.filter(command => command === "launch_agent")).toHaveLength(1);
  await user.click(screen.getByRole("link", { name: "ChatGPT / Codex" }));
  expect(await screen.findByRole("switch", { name })).not.toBeChecked();
});

it("offers one Apply to install media in an existing Claude Code gateway profile", async () => {
  const user = userEvent.setup();
  launch("all_agents", "/agents/claude", ({ agents }) => {
    const claude = agents.value.claude;
    if (claude.status !== "found") throw new Error("Claude Code fixture missing");
    agents.publish({ ...agents.value, claude: { ...claude, data: { ...claude.data, media_needs_apply: true } } });
  });
  await user.click(await screen.findByRole("switch", { name: "Подключить coreinfra-media для видео, озвучки и распознавания речи" }));
  expect(screen.getByText(/локальный MCP-сервер/)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Применить" }));
  const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Code" });
  await user.click(within(plan).getByRole("button", { name: "Применить" }));
  await finishSettings(user);
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
});

it("applies a Claude Desktop media change through a plan that restarts Desktop, then saves the grant", async () => {
  const user = userEvent.setup();
  const prepare = vi.spyOn(agentCommands, "prepareAgentChange");
  try {
    launch("all_agents", "/agents/claude-desktop");
    const toggle = await screen.findByRole("switch", { name: /coreinfra-media/ });
    expect(toggle).not.toBeChecked();
    await user.click(toggle);
    await user.click(screen.getByRole("button", { name: "Применить" }));
    const plan = await screen.findByRole("dialog", { name: /План изменений/ });
    // A media-only plan can close Desktop without reconfiguring its gateway or agent token.
    expect(prepare).toHaveBeenLastCalledWith(expect.objectContaining({
      agent: "claude_desktop",
      routing: undefined,
      mediaEnabled: true,
    }));
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await screen.findByRole("dialog", { name: "Настройки применены" });
    const applied = environment.calls.indexOf("apply_agent_plan");
    expect(applied).toBeGreaterThanOrEqual(0);
    expect(environment.calls.indexOf("set_media_connection")).toBeGreaterThan(applied);
    await finishSettings(user);
    expect(await screen.findByRole("switch", { name: /coreinfra-media/ })).toBeChecked();
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
  } finally {
    prepare.mockRestore();
  }
});

it("keeps the Claude Desktop media choice across routing modes", async () => {
  const user = userEvent.setup();
  const prepare = vi.spyOn(agentCommands, "prepareAgentChange");
  try {
    launch("all_agents", "/agents/claude-desktop");
    await screen.findByRole("switch", { name: /coreinfra-media/ });
    await user.click(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" }));
    expect(screen.getByRole("switch", { name: /coreinfra-media/ })).toBeVisible();
    await user.click(screen.getByRole("radio", { name: "Своя подписка Claude" }));
    await user.click(await screen.findByRole("button", { name: "Пропустить" }));
    await user.click(await screen.findByRole("switch", { name: /coreinfra-media/ }));
    await user.click(screen.getByRole("button", { name: "Применить" }));
    await screen.findByRole("dialog", { name: /План изменений/ });
    expect(prepare).toHaveBeenLastCalledWith(expect.objectContaining({
      agent: "claude_desktop",
      routing: expect.objectContaining({ change: { ClaudeDesktopSetRouting: "sub" } }),
      mediaEnabled: true,
    }));
  } finally {
    prepare.mockRestore();
  }
});

it("remembers declining the default media offer while switching Claude Desktop away from Hub", async () => {
  const user = userEvent.setup();
  launch("all_agents", "/agents/claude-desktop", undefined, { mediaSuggested: true });
  const toggle = await screen.findByRole("switch", { name: /coreinfra-media/ });
  expect(toggle).toBeChecked();
  await user.click(toggle);
  await user.click(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" }));
  await user.click(screen.getByRole("button", { name: "Применить" }));
  const plan = await screen.findByRole("dialog", { name: /План изменений/ });
  expect(within(plan).getByText(/Отключит MCP-сервер/)).toBeVisible();
  await user.click(within(plan).getByRole("button", { name: "Применить" }));
  await finishSettings(user);
  await user.click(screen.getByRole("link", { name: "Claude Code" }));
  await user.click(screen.getByRole("link", { name: "Claude Desktop" }));
  expect(await screen.findByRole("switch", { name: /coreinfra-media/ })).not.toBeChecked();
  expect(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" })).toBeChecked();
  expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
});

it("migrates an outdated Claude Desktop configuration without changing the selected mode", async () => {
  const user = userEvent.setup();
  launch("all_agents", "/agents/claude-desktop", ({ agents }) => {
    const desktop = agents.value.claude_desktop;
    if (desktop.status !== "found") throw new Error("Expected installed desktop");
    agents.publish({ ...agents.value, claude_desktop: {
      ...desktop, data: { ...desktop.data, configuration_needs_apply: true },
    } });
  });
  const apply = await screen.findByRole("button", { name: "Применить" });
  await waitFor(() => expect(apply).toBeEnabled());
  await user.click(apply);
  const plan = await screen.findByRole("dialog", { name: /План изменений/ });
  await user.click(within(plan).getByRole("button", { name: "Применить" }));
  await finishSettings(user);
  expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
  const desktop = environment.agents.value.claude_desktop;
  expect(desktop.status === "found" && desktop.data.routing_mode).toBe("hub");
});

it("saves both media choices in Claude Desktop without a Hub connection", async () => {
  const user = userEvent.setup();
  launch("offline", "/agents/claude-desktop", ({ agents }) => {
    const desktop = agents.value.claude_desktop;
    if (desktop.status !== "found") throw new Error("Expected installed desktop");
    agents.publish({ ...agents.value, claude_desktop: {
      ...desktop, data: { ...desktop.data, routing_mode: "disabled", token: null },
    } });
  }, { mediaSuggested: true });
  for (const enabled of [false, true]) {
    const toggle = await screen.findByRole("switch", { name: /coreinfra-media/ });
    expect(toggle).toHaveAttribute("aria-checked", String(!enabled));
    await user.click(toggle);
    const apply = screen.getByRole("button", { name: "Применить" });
    expect(apply).toBeEnabled();
    await user.click(apply);
    const plan = await screen.findByRole("dialog", { name: /План изменений/ });
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    expect(await screen.findByRole("switch", { name: /coreinfra-media/ })).toHaveAttribute("aria-checked", String(enabled));
    expect(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" })).toBeChecked();
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
  }
});

it("offers media by default to configured agents until the user turns it off", async () => {
  const user = userEvent.setup();
  launch("all_agents", "/agents/claude", undefined, { mediaSuggested: true });
  const toggle = await screen.findByRole("switch", { name: /coreinfra-media/ });
  expect(toggle).toBeChecked();
  const apply = screen.getByRole("button", { name: "Применить" });
  expect(apply).toBeEnabled();
  await user.click(toggle);
  expect(apply).toBeEnabled();
  await user.click(toggle);
  await user.click(apply);
  let plan = await screen.findByRole("dialog", { name: "План изменений: Claude Code" });
  expect(within(plan).getByText("coreinfra_media", { selector: "code" })).toBeVisible();
  await user.click(within(plan).getByRole("button", { name: "Применить" }));
  await finishSettings(user);
  expect(await screen.findByRole("switch", { name: /coreinfra-media/ })).toBeChecked();

  // Turning media off is remembered: it is not offered again.
  await user.click(screen.getByRole("switch", { name: /coreinfra-media/ }));
  await user.click(screen.getByRole("button", { name: "Применить" }));
  plan = await screen.findByRole("dialog", { name: "План изменений: Claude Code" });
  await user.click(within(plan).getByRole("button", { name: "Применить" }));
  await finishSettings(user);
  await user.click(screen.getByRole("link", { name: "ChatGPT / Codex" }));
  await user.click(screen.getByRole("link", { name: "Claude Code" }));
  expect(await screen.findByRole("switch", { name: /coreinfra-media/ })).not.toBeChecked();
  expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
});

it("offers Apply to migrate legacy media registration in an upgraded Codex without changing the choice", async () => {
  const user = userEvent.setup();
  launch("all_agents", "/agents/codex", ({ agents }) => {
    const codex = agents.value.codex;
    if (codex.status !== "found") throw new Error("Codex fixture missing");
    agents.publish({ ...agents.value, codex: { ...codex, data: { ...codex.data, media_needs_apply: true } } });
  });
  const toggle = await screen.findByRole("switch", { name: /coreinfra-media/ });
  expect(toggle).not.toBeChecked();
  const save = vi.spyOn(agentCommands, "applyMediaConnection");
  try {
    await user.click(screen.getByRole("button", { name: "Применить" }));
    const plan = await screen.findByRole("dialog", { name: "План изменений: ChatGPT / Codex" });
    expect(within(plan).getByText("coreinfra_media", { selector: "code" })).toBeVisible();
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    expect(save).toHaveBeenCalledExactlyOnceWith("codex", false);
    expect(environment.calls).not.toContain("apply_agent_plan");
  } finally {
    save.mockRestore();
  }
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
});

it("keeps the Claude Code media choice in all three modes and retries only the failed step", async () => {
  const user = userEvent.setup();
  launch("all_agents", "/agents/claude");
  const toggle = await screen.findByRole("switch", { name: /coreinfra-media/ });
  expect(toggle).not.toBeChecked();
  await user.click(toggle);
  await user.click(screen.getByRole("radio", { name: "Своя подписка Claude" }));
  await user.click(await screen.findByRole("button", { name: "Пропустить" }));
  expect(toggle).toBeVisible();
  expect(toggle).toBeChecked();
  await user.click(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" }));
  expect(toggle).toBeVisible();
  expect(toggle).toBeChecked();
  await user.click(screen.getByRole("button", { name: "Применить" }));
  const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Code" });
  expect(within(plan).getByText("coreinfra_media", { selector: "code" })).toBeVisible();
  const details = within(plan).getByRole("button", { name: "Ещё 1 шаг" });
  await user.click(details);
  expect(within(plan).getByText("Обновит режим подключения и токен CoreInfra Hub.")).toBeVisible();
  await user.keyboard("{Enter}");
  expect(details).toHaveAttribute("aria-expanded", "false");
  const save = vi.spyOn(agentCommands, "applyMediaConnection").mockRejectedValueOnce("Сервис занят");
  try {
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    expect(await within(plan).findByRole("alert")).toHaveTextContent("Сервис занят");
    expect(screen.queryByRole("dialog", { name: "Настройки применены" })).not.toBeInTheDocument();
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    expect(save).toHaveBeenCalledTimes(2);
    expect(environment.calls.filter(command => command === "apply_agent_plan")).toHaveLength(1);
    expect(environment.calls).not.toContain("launch_agent");
  } finally {
    save.mockRestore();
  }
});

describe("shared installation flow", () => {
  it("keeps a download through Back and Escape, then cancels and starts again from zero", async () => {
    const user = userEvent.setup();
    launch("update", "/overview", undefined, { onboarding: true });
    const picker = await openInstallationPicker(user);
    await user.click(within(picker).getAllByRole("button", { name: "Обновить" })[0]);
    let dialog = await screen.findByRole("dialog", {
      name: "Установка агента",
    });
    expect(environment.calls).not.toContain("download_desktop_release");
    await user.click(within(dialog).getByRole("button", { name: "Скачать обновление" }));
    await user.click(within(dialog).getByRole("button", { name: "Назад" }));
    dialog = await screen.findByRole("dialog", { name: "Установите агентов" });
    expect(
      within(dialog).getByRole("progressbar", {
        name: "Скачивание Claude Desktop",
      }),
    ).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(environment.installs.claude.value.status).toBe("downloading");
    await user.click(
      screen.getByRole("button", { name: "Открыть установку Claude Desktop" }),
    );
    dialog = await screen.findByRole("dialog", { name: "Установка агента" });
    expect(within(dialog).queryByRole("button", { name: "Назад" })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Отменить" }));
    await within(dialog).findByRole("button", { name: "Скачать обновление" });
    expect(environment.installs.claude.value.status).toBe("available");
    expect(environment.calls).toContain("cancel_desktop_download");
    expect(environment.calls).not.toContain("install_desktop_release");
    await user.click(
      within(dialog).getByRole("button", { name: "Скачать обновление" }),
    );
    expect(environment.installs.claude.value).toMatchObject({
      status: "downloading",
      received: 0,
    });
  });

  it("opens a direct update without a fictitious previous screen", async () => {
    const user = userEvent.setup();
    launch();
    await user.click(await screen.findByRole("button", { name: "Обновить" }));
    const dialog = await screen.findByRole("dialog", { name: "Установка агента" });
    expect(within(dialog).queryByRole("button", { name: "Назад" })).not.toBeInTheDocument();
    expect(environment.calls).not.toContain("download_desktop_release");
    await user.click(within(dialog).getByRole("button", { name: "Скачать обновление" }));
    await user.click(within(dialog).getByRole("button", { name: "Отменить" }));
    await user.click(await within(dialog).findByRole("button", { name: "Скачать обновление" }));
    expect(within(dialog).queryByRole("button", { name: "Назад" })).not.toBeInTheDocument();
  });

  it("keeps two operations independent when one is canceled", async () => {
    const user = userEvent.setup();
    launch("downloads", "/overview");
    await user.click(
      await screen.findByRole("button", {
        name: "Открыть установку Claude Desktop",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Отменить" }));
    expect(environment.installs.claude.value.status).toBe("available");
    expect(environment.installs.chatgpt.value.status).toBe("downloading");
  });

  it("requires the explicit confirmation before installing a replacement", async () => {
    const user = userEvent.setup();
    launch("confirmation");
    await user.click(await screen.findByRole("button", { name: "Обновить" }));
    const dialog = await screen.findByRole("dialog");
    const install = within(dialog).getByRole("button", { name: "Установить" });
    expect(install).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox"));
    await waitFor(() => expect(install).toBeEnabled());
    await user.click(install);
    expect(environment.installs.claude.value.status).toBe("installing");
    await within(dialog).findByRole(
      "button",
      { name: "Готово" },
      { timeout: 3000 },
    );
    expect(
      environment.calls.filter(
        (command) => command === "install_desktop_release",
      ),
    ).toHaveLength(1);
  });

  it("offers installation after completion without automatically installing", async () => {
    const user = userEvent.setup();
    launch();
    await user.click(
      await screen.findByRole("button", { name: "Обновить" }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Скачать обновление" }));
    await act(async () => environment.finishDownloads());
    await within(dialog).findByRole("button", { name: "Установить" });
    expect(
      within(dialog).queryByText("Версия 1.0.121"),
    ).not.toBeInTheDocument();
    expect(
      within(dialog).getByText("Версия 1.0.124 — готова к установке"),
    ).toBeInTheDocument();
    expect(environment.calls).not.toContain("install_desktop_release");
  });

  it("discards a completed archive from the dialog and offers a fresh download", async () => {
    const user = userEvent.setup();
    launch("ready");
    await user.click(await screen.findByRole("button", { name: "Обновить" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Отменить" }));
    await within(dialog).findByRole("button", { name: "Скачать обновление" });
    expect(environment.installs.claude.value).toMatchObject({
      status: "available",
      saved: null,
    });
    expect(environment.calls).not.toContain("install_desktop_release");
    expect(environment.installs.chatgpt.value.status).toBe("ready_to_install");
  });

  it("opens a ready picker card without installing or canceling its archive", async () => {
    const user = userEvent.setup();
    launch("ready", "/overview", undefined, { onboarding: true });
    const dialog = await openInstallationPicker(user);
    const card = within(dialog).getByRole("article", { name: "ChatGPT" });
    expect(within(card).queryByRole("button", { name: /Отменить|Установить/ })).not.toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: "Обновить" }));
    const installation = await screen.findByRole("dialog", { name: "Установка агента" });
    expect(within(installation).getByRole("button", { name: "Отменить" })).toBeInTheDocument();
    expect(within(installation).getByRole("button", { name: "Установить" })).toBeInTheDocument();
    expect(environment.installs.claude.value.status).toBe("ready_to_install");
    expect(environment.installs.chatgpt.value.status).toBe("ready_to_install");
    expect(environment.calls).not.toContain("cancel_desktop_download");
    expect(environment.calls).not.toContain("install_desktop_release");
  });

  it("keeps the installed version until the update is applied through its panel", async () => {
    const user = userEvent.setup();
    launch("ready");
    const update = await screen.findByRole("button", { name: "Обновить" });
    const versionRow = update.closest(".ui-version-row")! as HTMLElement;
    expect(within(versionRow).getByText("v1.0.121")).toBeInTheDocument();
    expect(within(versionRow).queryByRole("button", { name: /Установить|Отменить/ })).not.toBeInTheDocument();
    expect(within(versionRow).queryByText(/Готова к установке/)).not.toBeInTheDocument();
    await user.click(update);
    const dialog = await screen.findByRole("dialog", { name: "Установка агента" });
    expect(environment.calls).not.toContain("install_desktop_release");
    expect(environment.calls).not.toContain("cancel_desktop_download");
    await user.click(await within(dialog).findByRole("button", { name: "Установить" }));
    await within(dialog).findByRole("button", { name: "Готово" }, { timeout: 3000 });
    await user.click(within(dialog).getByRole("button", { name: "Готово" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(within(versionRow).getByText("v1.0.124")).toBeInTheDocument();
    expect(within(versionRow).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByText("Версия актуальна")).not.toBeInTheDocument();
  });

  it("shows current versions without manual update checks", async () => {
    const user = userEvent.setup();
    launch("current", "/overview", undefined, { onboarding: true });
    const dialog = await openInstallationPicker(user);
    expect(within(dialog).getByText("Версия 1.0.124")).toBeInTheDocument();
    expect(within(dialog).getByText("Версия 1.2026.321")).toBeInTheDocument();
    expect(within(dialog).queryByText("Версия актуальна")).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Обновить" })).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Проверить обновления/ }),
    ).not.toBeInTheDocument();
  });

  it.each(["Claude Desktop", "ChatGPT"])("keeps a first %s installation unconfigured until the connection is applied", async (name) => {
    const user = userEvent.setup();
    launch("missing", "/overview");
    const picker = await openInstallationPicker(user);
    await user.click(within(within(picker).getByRole("article", { name })).getByRole("button", { name: "Скачать" }));
    const dialog = await screen.findByRole("dialog", { name: "Установка агента" });
    await user.click(within(dialog).getByRole("button", { name: "Скачать" }));
    await act(async () => environment.finishDownloads());
    const install = await within(dialog).findByRole("button", { name: "Установить" });
    await waitFor(() => expect(install).toBeEnabled());
    await user.click(install);
    await user.click(await within(dialog).findByRole("button", { name: "Готово" }, { timeout: 3000 }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const agent = environment.agents.value[name === "Claude Desktop" ? "claude_desktop" : "codex"];
    expect(agent).toMatchObject({ status: "found", data: { routing_mode: "disabled", token: null } });
    expect(screen.getByRole("button", { name: "Настроить агента" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Панель управления" })).not.toBeInTheDocument();
  });

  it("preserves the chosen connection and token when updating a desktop app", async () => {
    const user = userEvent.setup();
    launch("ready", "/agents/claude-desktop", (mock) => {
      const desktop = mock.agents.value.claude_desktop;
      if (desktop.status !== "found") throw new Error("Expected installed desktop");
      mock.agents.publish({ ...mock.agents.value, claude_desktop: {
        ...desktop,
        data: { ...desktop.data, routing_mode: "sub", token: "demo-team-token-not-a-credential" },
      } });
    });
    await user.click(await screen.findByRole("button", { name: "Обновить" }));
    const dialog = await screen.findByRole("dialog", { name: "Установка агента" });
    const install = await within(dialog).findByRole("button", { name: "Установить" });
    await waitFor(() => expect(install).toBeEnabled());
    await user.click(install);
    await within(dialog).findByRole("button", { name: "Готово" }, { timeout: 3000 });
    expect(environment.agents.value.claude_desktop).toMatchObject({
      status: "found",
      data: { version: "1.0.124", routing_mode: "sub", token: "demo-team-token-not-a-credential" },
    });
  });
});

describe("application transitions", () => {
  it("opens the Wizard updater without starting it, preserves downloads on close and installs only on request", async () => {
    const user = userEvent.setup();
    launch("wizard_update", "/overview");
    await user.click(await screen.findByRole("button", { name: "Обновить до v1.0.6" }));
    let dialog = await screen.findByRole("dialog", { name: "Обновление Wizard" });
    expect(environment.calls).not.toContain("request_update");
    expect(within(dialog).queryByRole("button", { name: "Назад" })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Скачать обновление" }));
    await within(dialog).findByRole("progressbar", { name: "Скачивание Wizard" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(environment.update.value.status).toBe("downloading");
    expect(screen.getByRole("region", { name: "Хаб и агенты" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Открыть установку Wizard" }));
    dialog = await screen.findByRole("dialog", { name: "Обновление Wizard" });
    await user.click(within(dialog).getByRole("button", { name: "Отменить" }));
    await user.click(await within(dialog).findByRole("button", { name: "Скачать обновление" }));
    expect(environment.update.value).toMatchObject({ status: "downloading", received: 0 });
    await act(async () => environment.finishDownloads());
    const install = await within(dialog).findByRole("button", { name: "Установить" });
    expect(environment.calls).not.toContain("install_update");
    await user.click(install);
    await within(dialog).findByText("Устанавливается…");
    expect(document.querySelector(".overview")).toBeInTheDocument();
    await within(dialog).findByRole("button", { name: "Готово" }, { timeout: 2000 });
    expect(environment.calls).toContain("install_update");
  });

  it("replaces getting started with the overview after the first connection is configured", async () => {
    const user = userEvent.setup();
    launch("onboarding_merge", "/overview", undefined, { mediaSuggested: true });
    await screen.findByRole("heading", { name: "Начало работы" });
    expect(screen.queryByRole("button", { name: "Пополнить" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Настроить агента" }));
    const dialog = await screen.findByRole("dialog", { name: "Настройка агентов" });
    await user.click(within(dialog).getByRole("radio", { name: "Claude Desktop" }));
    await user.click(within(dialog).getByRole("button", { name: "Настроить агента" }));
    const media = await within(dialog).findByRole("switch", { name: /coreinfra-media/ });
    expect(media).toBeChecked();
    await user.click(media);
    await user.click(await within(dialog).findByRole("button", { name: "Завершить настройку" }));
    const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Desktop" });
    await user.click(within(plan).getByRole("button", { name: "Завершить настройку" }));
    await finishSettings(user);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(window.location.hash).toBe("#/overview");
    const dashboard = await screen.findByRole("region", { name: "Хаб и агенты" });
    expect(within(dashboard).getAllByRole("article")).toHaveLength(2);
    expect(within(dashboard).getByRole("button", { name: "Панель управления" })).toBeInTheDocument();
    expect(within(dashboard).queryByRole("button", { name: /Установить/ })).not.toBeInTheDocument();
    expect(within(dashboard).getByText("Приложения, обновления и настройки подключения.")).toBeInTheDocument();
    await screen.findByRole("heading", { name: "Агент готов!" });
    expect(screen.queryByRole("heading", { name: "Начало работы" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "Claude Desktop" }));
    expect(await screen.findByRole("switch", { name: /coreinfra-media/ })).not.toBeChecked();
    await waitFor(() => expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled());
  });

  it("does not mistake a detection failure for missing applications on the overview", async () => {
    const user = userEvent.setup();
    launch("agents_error", "/overview");
    await screen.findByText("Не все приложения удалось проверить.");
    await user.click(screen.getByRole("button", { name: "Панель управления" }));
    await screen.findByText(/Не удалось проверить приложения/);
    expect(screen.queryByText("Установите приложение и подключите его к хабу.")).not.toBeInTheDocument();
  });

  it.each(["desktop", "cli"] as const)("shows a nested Codex %s detection error even with routing disabled", async (part) => {
    const user = userEvent.setup();
    launch("unconfigured", "/overview", ({ agents }) => {
      const codex = agents.value.codex;
      if (codex.status !== "found") throw new Error("Codex fixture missing");
      agents.publish({ ...agents.value, codex: {
        ...codex,
        data: { ...codex.data, [part]: { status: "error", data: "Не удалось прочитать сведения о приложении." } },
      } });
    });
    await user.click(await screen.findByRole("button", { name: "Панель управления" }));
    const region = await screen.findByRole("region", { name: "Агенты" });
    const row = within(region).getByRole("link", { name: "ChatGPT / Codex" }).closest("li")!;
    expect(within(row).getByText("Не удалось проверить")).toBeInTheDocument();
    expect(within(row).queryByText("Не установлен")).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /Скачать/ })).not.toBeInTheDocument();
    await user.click(within(row).getByRole("button", { name: "Подробнее о ChatGPT / Codex" }));
    await screen.findByText(/Не удалось обнаружить: Не удалось прочитать сведения о приложении/);
  });

  it("treats disabled Claude routing and a stopped proxy as a normal state", async () => {
    const user = userEvent.setup();
    launch("unconfigured");
    await screen.findByText("Остановлен");
    expect(screen.queryByText(/Примените подключение заново/)).not.toBeInTheDocument();
    expect(environment.calls).not.toContain("ensure_agent_proxy");
    expect(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" })).toBeChecked();
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: "API-доступ" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Применить" })).toBeEnabled());
  });

  it("shows configured agents on the overview and opens settings directly", async () => {
    const user = userEvent.setup();
    launch("all_agents", "/overview");
    const dashboard = await screen.findByRole("region", { name: "Хаб и агенты" });
    const preview = within(dashboard).getByRole("list", { name: "Приложения" });
    expect(within(preview).getAllByRole("listitem").map(item => item.textContent)).toEqual(["Claude Desktop", "ChatGPT", "… ещё 4"]);
    expect(within(dashboard).queryByText(/Подключено агентов/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Посмотреть видео о Wizard" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Панель управления" }));
    await screen.findByRole("region", { name: "Агенты" });
    expect(screen.queryByText("Начало работы")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Обновить" })).toHaveLength(2);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await user.click(screen.getByRole("link", { name: "Настройки" }));
    await screen.findByRole("heading", { name: "Настройки", level: 1 });
    expect(screen.getByRole("link", { name: "Настройки" })).toHaveAttribute("aria-current", "page");
  });

  it("opens a specific update from management and returns to the list on close", async () => {
    const user = userEvent.setup();
    launch("update", "/overview");
    await user.click(await screen.findByRole("button", { name: "Панель управления" }));
    const management = await screen.findByRole("dialog", { name: "Управление агентами" });
    expect(within(management).queryByRole("button", { name: /Установить агента|Настроить агента/ })).not.toBeInTheDocument();
    const row = within(management).getByRole("link", { name: "Claude Desktop" }).closest("li")!;
    await user.click(within(row).getByRole("button", { name: "Обновить" }));
    const installation = await screen.findByRole("dialog", { name: "Установка агента" });
    expect(within(installation).queryByRole("button", { name: "Назад" })).not.toBeInTheDocument();
    expect(environment.calls).not.toContain("download_desktop_release");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Установка агента" })).not.toBeInTheDocument());
    expect(screen.getByRole("dialog", { name: "Управление агентами" })).toBeVisible();
    expect(window.location.hash).toBe("#/overview");
  });

  it("edits connection settings from the status inside management and discards drafts on Back", async () => {
    const user = userEvent.setup();
    launch("all_agents", "/overview");
    await user.click(await screen.findByRole("button", { name: "Панель управления" }));
    const management = await screen.findByRole("dialog", { name: "Управление агентами" });
    await user.click(within(management).getByRole("button", { name: "Настройки подключения Claude Desktop: CoreInfra AI Hub" }));
    expect(within(management).getByRole("radio", { name: "API-доступ" })).toBeChecked();
    await user.click(within(management).getByRole("radio", { name: "Без CoreInfra AI Hub" }));
    await user.click(within(management).getByRole("button", { name: "Назад" }));
    const status = within(management).getByRole("button", { name: "Настройки подключения Claude Desktop: CoreInfra AI Hub" });
    await waitFor(() => expect(status).toHaveFocus());
    expect(environment.calls).not.toContain("prepare_agent_change");
    await user.click(status);
    expect(within(management).getByRole("radio", { name: "API-доступ" })).toBeChecked();
    await user.click(within(management).getByRole("radio", { name: "Без CoreInfra AI Hub" }));
    await user.click(within(management).getByRole("button", { name: "Применить" }));
    const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Desktop" });
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "План изменений: Claude Desktop" })).not.toBeInTheDocument());
    const updated = await within(management).findByRole("button", { name: "Настройки подключения Claude Desktop: Без CoreInfra" });
    await waitFor(() => expect(updated).toHaveFocus());
    expect(within(management).queryByRole("radio")).not.toBeInTheDocument();
    expect(environment.calls.filter(command => command === "apply_agent_plan")).toHaveLength(1);
    expect(window.location.hash).toBe("#/overview");
  });

  it("shows connection settings immediately without a collapsed summary or cancel action", async () => {
    launch();
    expect(await screen.findByRole("radio", { name: "API-доступ" })).toBeChecked();
    expect(screen.queryByRole("button", { name: "Настроить подключение" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Отмена" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
  });

  it("opens the selected hub's payment page and removes the prompt after topping up", async () => {
    const user = userEvent.setup();
    launch("balance_empty", "/overview");
    await screen.findByRole("heading", { name: "Агент готов!" });
    expect(screen.getByText("0 ₽")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Пополнить" }));
    expect(environment.openedHubPages).toEqual([{ hubId: "preview-hub", section: "payments" }]);
    await screen.findByText("1 500 ₽");
    expect(screen.queryByRole("heading", { name: "Агент готов!" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Хаб и агенты" })).toBeInTheDocument();
  });

  it.each([
    ["balance_guest", null],
    ["balance_error", "Не удалось загрузить баланс"],
    ["update", "2 500 ₽"],
  ] as const)("does not ask for money in the %s state", async (scenario, label) => {
    launch(scenario, "/overview");
    if (label) await screen.findByText(label);
    else {
      await screen.findByRole("region", { name: "Хаб и агенты" });
      await waitFor(() => expect(screen.queryByText("Баланс хаба")).not.toBeInTheDocument());
      expect(screen.queryByText("Доступ к хабу")).not.toBeInTheDocument();
      expect(screen.queryByText("Гостевой доступ")).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Перейти в хаб" })).toBeInTheDocument();
    }
    expect(screen.queryByRole("button", { name: "Пополнить" })).not.toBeInTheDocument();
    expect(screen.queryByText("0 ₽")).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Хаб и агенты" })).toBeInTheDocument();
  });

  it("does not treat a hidden balance as zero", async () => {
    launch("balance_empty", "/overview", ({ summary }) => summary.publish({ ...summary.value, role_in_hub: "member", balance_rub: null }));
    await screen.findByText("Баланс недоступен");
    expect(screen.queryByRole("button", { name: "Пополнить" })).not.toBeInTheDocument();
  });

  it("discards the old hub's balance immediately when switching hubs", async () => {
    launch("balance_empty", "/overview");
    await screen.findByRole("button", { name: "Пополнить" });
    await act(async () => environment.account.publish({ ...environment.account.value, active_hub_id: "team-hub" }));
    await screen.findByText("Не удалось загрузить баланс");
    expect(screen.queryByRole("button", { name: "Пополнить" })).not.toBeInTheDocument();
    expect(screen.queryByText("0 ₽")).not.toBeInTheDocument();
  });

  it("copies an installation command for a missing agent without executing it", async () => {
    const user = userEvent.setup();
    launch("missing", "/agents/claude");
    await screen.findByRole("heading", { name: "Claude Code не установлен" });
    await user.click(screen.getByRole("button", { name: "Скопировать команду" }));
    expect(await navigator.clipboard.readText()).toBe("npm install -g @anthropic-ai/claude-code");
    expect(screen.getByRole("button", { name: "Скопировано" })).toBeInTheDocument();
    expect(environment.calls).not.toContain("install_desktop_release");
  });

  it("enables Apply only for a changed connection and disables it when the original mode is restored", async () => {
    const user = userEvent.setup();
    launch();
    const apply = await screen.findByRole("button", { name: "Применить" });
    expect(apply).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" }));
    expect(apply).toBeEnabled();
    await user.click(screen.getByRole("radio", { name: "API-доступ" }));
    expect(apply).toBeDisabled();
    expect(screen.queryByText(/установит скилл/)).not.toBeInTheDocument();
    expect(environment.calls).not.toContain("prepare_agent_change");
    expect(environment.calls).not.toContain("apply_agent_plan");
  });

  it("keeps the connection form open and disables Apply after saving a change", async () => {
    const user = userEvent.setup();
    launch();
    await user.click(await screen.findByRole("radio", { name: "Без CoreInfra AI Hub" }));
    await user.click(screen.getByRole("button", { name: "Применить" }));
    const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Desktop" });
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const savedMode = screen.getByRole("radio", { name: "Без CoreInfra AI Hub" });
    expect(savedMode).toBeChecked();
    await waitFor(() => expect(savedMode).toHaveFocus());
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Отмена" })).not.toBeInTheDocument();
    expect(environment.calls.filter((command) => command === "apply_agent_plan")).toHaveLength(1);
  });

  it.each([
    { label: "API-доступ", mode: "hub" },
    { label: "Своя подписка Claude", mode: "sub" },
  ])("saves the selected mock key for $label and keeps it across mode changes", async ({ label, mode }) => {
    const user = userEvent.setup();
    launch("update", "/agents/claude-desktop", ({ systemProxy }) => {
      systemProxy.publish({ ...systemProxy.value, status: "on" });
    });
    await user.click(await screen.findByRole("radio", { name: label }));
    await user.click(screen.getByRole("button", { name: "Изменить токен" }));
    await user.click(await screen.findByRole("menuitem", { name: /Команда · демо/ }));
    expect(screen.getByText(/Будет использован токен «Команда · демо»/)).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: "Без CoreInfra AI Hub" }));
    expect(screen.queryByRole("button", { name: "Изменить токен" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: mode === "sub" ? "API-доступ" : "Своя подписка Claude" }));
    expect(screen.getByText(/Будет использован токен «Команда · демо»/)).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: label }));
    await user.click(screen.getByRole("button", { name: "Применить" }));
    const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Desktop" });
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(environment.agents.value.claude_desktop).toMatchObject({
      status: "found",
      data: { routing_mode: mode, token: "demo-team-token-not-a-credential" },
    });
    expect(screen.getByRole("radio", { name: label })).toBeChecked();
    expect(screen.getByText(/Будет использован токен «Команда · демо»/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
  });

  it("opens a download from its sidebar widget and preserves both transfers on close", async () => {
    const user = userEvent.setup();
    launch("downloads", "/overview");
    expect(screen.queryByRole("button", { name: /^Открыть загрузки/ })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Открыть установку Claude Desktop" }));
    const dialog = await screen.findByRole("dialog", { name: "Установка агента" });
    expect(within(dialog).getAllByRole("progressbar")).toHaveLength(1);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(environment.installs.claude.value.status).toBe("downloading");
    expect(environment.installs.chatgpt.value.status).toBe("downloading");
    await act(async () => environment.finishDownloads());
    for (const app of ["Claude Desktop", "ChatGPT"]) {
      expect(screen.getByRole("button", { name: `Открыть установку ${app}` })).toHaveTextContent("Готово к установке");
    }
  });

  it("keeps native Claude subscriptions on the shared proxy outside the mock preview", async () => {
    const user = userEvent.setup();
    const app = launch("update", "/agents/claude-desktop", ({ systemProxy }) => {
      systemProxy.publish({ ...systemProxy.value, status: "on" });
    });
    app.rerender(<App />);
    await user.click(await screen.findByRole("radio", { name: "Своя подписка Claude" }));
    expect(screen.getByText("Подключение через общий прокси Wizard.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Изменить токен" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Применить" }));
    const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Desktop" });
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(environment.agents.value.claude_desktop).toMatchObject({
      status: "found", data: { routing_mode: "sub", token: null },
    });
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
  });

  it("creates and retains a mock key when the hub has no keys", async () => {
    const user = userEvent.setup();
    launch("update", "/agents/claude-desktop", ({ tokens }) => {
      tokens.publish({ ...tokens.value, tokens: [] });
    });
    await screen.findByText(/Будет создан и использован токен «agent-claude-desktop»/);
    await user.click(screen.getByRole("button", { name: "Применить" }));
    const plan = await screen.findByRole("dialog", { name: "План изменений: Claude Desktop" });
    expect(environment.tokens.value.tokens).toHaveLength(1);
    await user.click(within(plan).getByRole("button", { name: "Применить" }));
    await finishSettings(user);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(environment.agents.value.claude_desktop).toMatchObject({
      status: "found", data: { token: environment.tokens.value.tokens[0].token },
    });
    expect(screen.getByRole("button", { name: "Применить" })).toBeDisabled();
  });

  it("keeps each download's progress independent when the other finishes", async () => {
    const user = userEvent.setup();
    launch("update", "/overview", ({ installs }) => {
      installs.claude.publish({ status: "downloading", received: 74, total: 100, reconnecting: false });
      installs.chatgpt.publish({ status: "downloading", received: 99, total: 100, reconnecting: false });
    });
    const claude = await screen.findByRole("button", { name: "Открыть установку Claude Desktop" });
    const chatgpt = screen.getByRole("button", { name: "Открыть установку ChatGPT" });
    expect(within(claude).getByText("74%")).toBeInTheDocument();
    expect(within(chatgpt).getByText("99%")).toBeInTheDocument();
    await act(async () => environment.installs.chatgpt.publish({
      status: "ready_to_install", version: "1.2026.321", path: "/Applications/ChatGPT.app", notes: [], confirmation: null,
    }));
    expect(within(claude).getByText("74%")).toBeInTheDocument();
    expect(within(chatgpt).queryByText("99%")).not.toBeInTheDocument();
    expect(chatgpt).toHaveTextContent("Готово к установке");
    await user.click(claude);
    const dialog = await screen.findByRole("dialog", { name: "Установка агента" });
    expect(within(dialog).getByRole("progressbar", { name: "Скачивание Claude Desktop" })).toHaveProperty("value", 74);
  });

  it("opens full navigation after the intro without requiring sign-in", async () => {
    const app = launch("startup", "/overview");
    expect(
      screen.getByRole("main", { name: "Запускаем Wizard" }),
    ).toBeInTheDocument();
    await screen.findByRole(
      "heading",
      { name: "Ура! Wizard установлен." },
      { timeout: 3000 },
    );
    expect(screen.getByRole("link", { name: "Обзор" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Войти" })).toBeInTheDocument();
    expect(localStorage.getItem("coreinfra.startup.seen")).toBe("true");
    app.unmount();
    render(<App />);
    expect(
      screen.queryByRole("main", { name: "Запускаем Wizard" }),
    ).not.toBeInTheDocument();
  });

  it("shows navigation while checking updates in the background", async () => {
    launch("checking");
    await screen.findByText("Проверяем версию…");
    expect(
      screen.getByRole("heading", { name: "Claude Desktop" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Обзор" })).toBeInTheDocument();
  });

  it("restores a setting on failure and keeps the retry available", async () => {
    const user = userEvent.setup();
    launch("settings_error", "/settings");
    const toggle = await screen.findByRole("switch", {
      name: "Работать в трее",
    });
    await user.click(toggle);
    await screen.findByText(
      "Не удалось сохранить настройку. Попробуйте ещё раз.",
    );
    expect(toggle).toBeChecked();
    expect(screen.getByRole("button", { name: "Повторить" })).toBeEnabled();
  });

  it("prevents overwriting another system proxy without an explicit action", async () => {
    launch("proxy_conflict", "/settings");
    const toggle = await screen.findByRole("switch", {
      name: "Использовать прокси Wizard",
    });
    expect(toggle).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Применить принудительно" }),
    ).toBeEnabled();
    expect(environment.calls).not.toContain("set_system_proxy");
  });
});
