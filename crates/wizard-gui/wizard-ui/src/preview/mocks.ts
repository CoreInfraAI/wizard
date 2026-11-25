import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type {
  AgentId,
  AgentChange,
  RoutingMode,
  PreparedAgentChange,
  AgentStates,
  TokenChoice,
} from "../agents/agents_state";
import type { DesktopApp, Phase } from "../agents/desktop_install_state";
import type { UpdateState } from "../updates/update";
import type { useSystemProxy } from "../settings/system_proxy";
import type { HubSummary } from "../settings/hub_summary";

export const scenarios = {
  startup: "Первый запуск",
  all_agents: "Все агенты",
  unconfigured: "Агенты без подключения",
  onboarding_merge: "Настройка → новый обзор",
  balance_empty: "Агент готов · нулевой баланс",
  balance_guest: "Гостевой доступ",
  balance_error: "Баланс недоступен",
  pending_login: "Вход в браузере",
  long_content: "Длинные названия и тексты",
  agents_error: "Ошибка обнаружения агентов",
  chatgpt_error: "Ошибка обнаружения ChatGPT",
  settings_error: "Ошибка сохранения настроек",
  proxy_conflict: "Конфликт системного прокси",
  empty_backups: "Нет резервных копий",
  install_error: "Ошибка установки",
  installed: "Установка завершена",
  checking: "Проверка обновлений в фоне",
  wizard_update: "Обновление Wizard",
  check_error: "Ошибка проверки обновлений",
  update: "Доступно обновление",
  downloads: "Две загрузки в панели",
  missing: "Агенты не установлены",
  current: "Актуальная версия",
  ready: "Скачано, можно установить",
  mixed_ready: "Скачано + доступно обновление",
  mixed_progress: "Скачано + идёт загрузка",
  mixed_error: "Ошибка загрузки + скачано",
  error: "Ошибка и повтор загрузки",
  reconnecting: "Переподключение",
  unknown: "Размер пока неизвестен",
  confirmation: "Подтверждение и открытые приложения",
  offline: "Без входа в аккаунт",
  unsupported: "Платформа не поддерживается",
} as const;
export type Scenario = keyof typeof scenarios;
const requestedScenario = new URLSearchParams(window.location.search).get(
  "scenario",
);
export const scenario: Scenario =
  requestedScenario && requestedScenario in scenarios
    ? (requestedScenario as Scenario)
    : "update";

class Snapshots<T extends object> {
  value: T & { revision: number };
  waiting = new Set<(value: T & { revision: number }) => void>();
  constructor(value: T) {
    this.value = { ...value, revision: 0 };
  }
  publish(value: T) {
    this.value = { ...value, revision: this.value.revision + 1 };
    this.waiting.forEach((resolve) => resolve(this.value));
    this.waiting.clear();
  }
  read(lastRevision?: number | null) {
    return lastRevision === this.value.revision
      ? new Promise<T & { revision: number }>((resolve) =>
          this.waiting.add(resolve),
        )
      : Promise.resolve(this.value);
  }
}

// `mediaSuggested`: Wizard offers media tools for every agent, as on a fresh install. Otherwise the
// fixtures model a user who has already turned media tools off, so other flows stay unchanged.
export type MockOptions = { onboarding?: boolean; mediaSuggested?: boolean };

export function createMockEnvironment(scenario: Scenario = "update", options: MockOptions = {}) {
  const timeouts = new Set<ReturnType<typeof setTimeout>>();
  const calls: string[] = [];
  const openedHubPages: { hubId: string; section: string }[] = [];
  let disposed = false;
  function later(action: () => void, ms: number) {
    const timer = setTimeout(() => {
      timeouts.delete(timer);
      if (!disposed) action();
    }, ms);
    timeouts.add(timer);
  }
  const versions = { claude: "1.0.124", chatgpt: "1.2026.321" };
  const previousVersions = { claude: "1.0.121", chatgpt: "1.2026.310" };
  const totals = { claude: 248 * 1048576, chatgpt: 186 * 1048576 };
  const token = "demo-preview-token-not-a-credential";
  const onboarding = options.onboarding || ["unconfigured", "onboarding_merge"].includes(scenario);
  const routing = {
    routing_mode: onboarding ? "disabled" as const : "hub" as const,
    token,
    proxy_needs_apply: false,
  };
  const found = scenario !== "missing" && scenario !== "startup";
  const detectedVersion =
    scenario === "current" || scenario === "installed"
      ? versions
      : previousVersions;
  const agents = new Snapshots<AgentStates>({
    claude_desktop: found
      ? {
          status: "found",
          data: {
            ...routing,
            configuration_needs_apply: false,
            media_needs_apply: false,
            path: "/Applications/Claude.app",
            version: detectedVersion.claude,
          },
        }
      : { status: "not_found" },
    codex: found
      ? {
          status: "found",
          data: {
            ...routing,
            media_needs_apply: false,
            cli: {
              status: "found",
              data: { path: "/usr/local/bin/codex", version: "0.112.0" },
            },
            desktop: scenario === "chatgpt_error" ? {
              status: "error",
              data: "Не удалось прочитать сведения о приложении ChatGPT.",
            } : {
              status: "found",
              data: {
                path: "/Applications/ChatGPT.app",
                version: detectedVersion.chatgpt,
              },
            },
          },
        }
      : { status: "not_found" },
    claude:
      scenario === "all_agents"
        ? {
            status: "found",
            data: {
              ...routing,
              media_needs_apply: false,
              path: "/usr/local/bin/claude",
              version: "2.1.0",
            },
          }
        : { status: "not_found" },
    opencode:
      scenario === "all_agents"
        ? {
            status: "found",
            data: {
              ...routing,
              path: "/usr/local/bin/opencode",
              version: "1.0.20",
            },
          }
        : { status: "not_found" },
    pi:
      scenario === "all_agents"
        ? {
            status: "found",
            data: { ...routing, path: "/usr/local/bin/pi", version: "0.56.0" },
          }
        : { status: "not_found" },
  });
  const account = new Snapshots({
    signed_in: !["offline", "startup", "pending_login"].includes(scenario),
    active_hub_id: scenario === "long_content" || scenario === "balance_guest" ? "team-hub" : "preview-hub",
    pending: scenario === "pending_login",
    error: null,
    profile: {
      personal_hub_id: "preview-hub",
      personal_hub_name: "preview",
      username:
        scenario === "long_content"
          ? "Команда разработки и исследования пользовательского интерфейса"
          : "UI Preview",
      verified_email:
        scenario === "long_content"
          ? "interface.research.and.development@example.com"
          : "preview@example.com",
      hubs: [
        {
          hub_id: "preview-hub",
          hub_name: "preview",
          display_name: "UI Preview",
          is_personal: true,
        },
        {
          hub_id: "team-hub",
          hub_name: "team",
          display_name: "Командный хаб разработки и исследований",
          is_personal: false,
        },
      ],
    },
  });
  const summary = new Snapshots<HubSummary>({
    hub_id: account.value.active_hub_id,
    role_in_hub: scenario === "balance_guest" ? "guest" : "owner",
    balance_rub: ["balance_empty", "balance_guest", "onboarding_merge"].includes(scenario) ? 0 : 2500,
  });
  const proxy = new Snapshots({ enabled: !onboarding });
  const mediaEnabled = new Set<string>();
  const mediaDeclined = new Set<string>(
    options.mediaSuggested ? [] : ["codex", "claude", "claude_desktop"],
  );
  const update = new Snapshots<UpdateState>(
    scenario === "wizard_update" ? { status: "available", version: "1.0.6" }
      : scenario === "checking" || scenario === "startup"
      ? { status: "checking" }
      : scenario === "check_error"
        ? {
            status: "failed",
            message: "Не удалось проверить обновления. Повторим автоматически.",
          }
        : { status: "up_to_date" },
  );
  let updateGeneration = 0;
  function downloadWizardUpdate() {
    if (update.value.status !== "available" && !(update.value.status === "failed" && update.value.version)) return;
    const generation = ++updateGeneration;
    const total = 84 * 1048576;
    let received = 0;
    const tick = () => {
      if (generation !== updateGeneration) return;
      received = Math.min(total, received + 4 * 1048576);
      update.publish(received === total ? { status: "ready_to_install", version: "1.0.6" }
        : { status: "downloading", version: "1.0.6", received, total });
      if (received < total) later(tick, 400);
    };
    update.publish({ status: "downloading", version: "1.0.6", received: 0, total });
    later(tick, 400);
  }
  const tokens = new Snapshots({
    hub_id: account.value.active_hub_id,
    tokens: [
      {
        token,
        name:
          scenario === "long_content"
            ? "Токен команды разработки и исследования пользовательского интерфейса"
            : "Агенты · демо",
        issued_at: 1780000000,
      },
      {
        token: "demo-team-token-not-a-credential",
        name: "Команда · демо",
        issued_at: 1780000100,
      },
    ],
    loading: false,
    error: null,
  });
  const application = new Snapshots({
    tray_enabled: true,
    autostart_enabled: false,
    supported: scenario !== "unsupported",
  });
  const systemProxy = new Snapshots<
    NonNullable<ReturnType<typeof useSystemProxy>["state"]>
  >({
    status: scenario === "unsupported" ? "unsupported" : "off",
    busy: false,
    conflict: scenario === "proxy_conflict",
    listener_running: true,
    read_error: null,
    operation_error: null,
  });
  const configured = new Snapshots({
    details: [{ name: "Агенты · демо", hub_id: "preview-hub" }],
    error: null,
  });

  function release(desktop: DesktopApp) {
    return {
      version: versions[desktop],
      path: `/Applications/${desktop === "claude" ? "Claude" : "ChatGPT"}.app`,
      notes: [],
      confirmation:
        scenario === "confirmation"
          ? "Я понимаю, что текущая версия приложения будет заменена"
          : null,
    };
  }
  function available(desktop: DesktopApp): Phase {
    return { status: "available", ...release(desktop), saved: null };
  }
  function initial(desktop: DesktopApp): Phase {
    if (scenario === "mixed_error" && desktop === "claude")
      return {
        status: "failed",
        operation: "download",
        message: "Соединение прервано. Повторите скачивание.",
        confirmation: null,
      };
    if (
      ((scenario === "mixed_ready" || scenario === "mixed_progress") &&
        desktop === "claude") ||
      (scenario === "mixed_error" && desktop === "chatgpt")
    )
      return { status: "ready_to_install", ...release(desktop) };
    if (scenario === "check_error")
      return {
        status: "failed",
        operation: "check",
        message: "Не удалось проверить версию. Повторим автоматически.",
        confirmation: null,
      };
    if (scenario === "installed")
      return { status: "installed", version: versions[desktop], notices: [] };
    if (scenario === "install_error")
      return {
        status: "failed",
        operation: "install",
        message:
          "Не удалось заменить приложение. Закройте его и повторите установку.",
        confirmation: null,
      };
    if (scenario === "current") return { status: "up_to_date" };
    if (scenario === "unsupported")
      return {
        status: "unsupported",
        message: "Установка недоступна на этой платформе.",
      };
    if (scenario === "ready" || scenario === "confirmation")
      return { status: "ready_to_install", ...release(desktop) };
    if (
      scenario === "offline" ||
      scenario === "startup" ||
      scenario === "checking"
    )
      return { status: "idle" };
    return available(desktop);
  }
  const installs = {
    claude: new Snapshots<Phase>(initial("claude")),
    chatgpt: new Snapshots<Phase>(initial("chatgpt")),
  };
  const timers: Partial<Record<DesktopApp, ReturnType<typeof setInterval>>> =
    {};
  const failedOnce = new Set<DesktopApp>();
  let speed = 1;
  function setDownloadSpeed(value: number) {
    speed = value;
  }
  function stop(desktop: DesktopApp) {
    clearInterval(timers[desktop]);
    delete timers[desktop];
  }
  function finish(desktop: DesktopApp) {
    stop(desktop);
    installs[desktop].publish({
      status: "ready_to_install",
      ...release(desktop),
    });
  }
  function finishDownloads() {
    if (update.value.status === "downloading") {
      updateGeneration++;
      update.publish({ status: "ready_to_install", version: "1.0.6" });
    }
    for (const desktop of ["claude", "chatgpt"] as const) {
      if (installs[desktop].value.status === "downloading") finish(desktop);
    }
  }

  function download(desktop: DesktopApp, initialPercent = 0) {
    const store = installs[desktop];
    if (
      store.value.status !== "available" &&
      !(store.value.status === "failed" && store.value.operation === "download")
    )
      throw "Сначала проверьте доступную версию";
    stop(desktop);
    let received = (totals[desktop] * initialPercent) / 100;
    let ticks = 0;
    store.publish({
      status: "downloading",
      received,
      total: scenario === "unknown" ? 0 : totals[desktop],
      reconnecting: false,
    });
    timers[desktop] = setInterval(() => {
      ticks++;
      if (scenario === "error" && ticks === 6 && !failedOnce.has(desktop)) {
        failedOnce.add(desktop);
        stop(desktop);
        store.publish({
          status: "failed",
          operation: "download",
          message:
            "Не удалось скачать приложение: соединение прервано. Повторите попытку.",
          confirmation: null,
        });
        return;
      }
      const reconnecting =
        scenario === "reconnecting" && ticks >= 6 && ticks <= 13;
      if (!reconnecting)
        received = Math.min(
          totals[desktop],
          received +
            (totals[desktop] / (scenario === "downloads" ? 240 : 72)) * speed,
        );
      if (received >= totals[desktop]) {
        finish(desktop);
        return;
      }
      store.publish({
        status: "downloading",
        received,
        total: scenario === "unknown" && ticks < 12 ? 0 : totals[desktop],
        reconnecting,
      });
    }, 500);
  }

  function checkRelease(desktop: DesktopApp) {
    const store = installs[desktop];
    if (["downloading", "installing", "checking"].includes(store.value.status))
      throw "Операция уже выполняется";
    store.publish({ status: "checking" });
    later(
      () =>
        store.publish(
          scenario === "current" ||
            (desktop === "claude"
              ? agents.value.claude_desktop.status === "found" &&
                agents.value.claude_desktop.data.version === versions.claude
              : agents.value.codex.status === "found" &&
                agents.value.codex.data.desktop.status === "found" &&
                agents.value.codex.data.desktop.data.version ===
                  versions.chatgpt)
            ? { status: "up_to_date" }
            : available(desktop),
        ),
      scenario === "checking" ? 3000 : 600,
    );
  }

  function checkIdleReleases() {
    if (!account.value.signed_in) return;
    for (const desktop of ["claude", "chatgpt"] as const) {
      if (installs[desktop].value.status === "idle") checkRelease(desktop);
    }
  }

  function start() {
    if (scenario === "startup") {
      // Replaying this mock deliberately simulates a fresh installation every time.
      try {
        localStorage.removeItem("coreinfra.startup.seen");
        localStorage.removeItem("coreinfra.onboarding.completed");
      } catch {
        /* Startup also works when storage is unavailable. */
      }
      window.location.hash = "/overview";
    }
    mockWindows("main");
    mockIPC(
      async (command, payload) => {
        if (disposed) return new Promise(() => {});
        calls.push(command);
        const args = (payload ?? {}) as Record<string, unknown>;
        const desktop = args.desktop as DesktopApp;
        const revision = args.lastRevision as number | null | undefined;
        switch (command) {
          case "get_agent_state":
            if (scenario === "agents_error")
              throw "Не удалось прочитать настройки агента";
            return agents.value;
          case "wait_for_update":
            return (await agents.read(revision)).revision;
          case "get_desktop_install_state":
            return installs[desktop].read(revision);
          case "get_hub_account_state":
            return account.read(revision);
          case "get_hub_summary":
            if (scenario === "balance_error") throw "Баланс временно недоступен";
            return summary.value;
          case "get_hub_tokens_state":
            return tokens.read(revision);
          case "get_configured_token_info":
            return configured.read(revision);
          case "get_proxy_state":
            return proxy.read(revision);
          case "get_media_connection": {
            const enabled = mediaEnabled.has(String(args.agent));
            return { enabled, suggested: !enabled && !mediaDeclined.has(String(args.agent)), running: enabled, error: null };
          }
          case "set_media_connection": {
            if (args.enabled) {
              mediaEnabled.add(String(args.agent));
              mediaDeclined.delete(String(args.agent));
            } else {
              mediaEnabled.delete(String(args.agent));
              mediaDeclined.add(String(args.agent));
            }
            const agent = args.agent as AgentId;
            const detection = agents.value[agent];
            if (detection.status === "found" && "media_needs_apply" in detection.data) {
              agents.publish({ ...agents.value, [agent]: {
                ...detection, data: { ...detection.data, media_needs_apply: false },
              } });
            }
            return;
          }
          case "get_update_state":
            return update.read(revision);
          case "get_application_settings":
            return application.read(revision);
          case "get_system_proxy_state":
            return systemProxy.read(revision);
          case "plugin:app|version":
            return "1.0.5-preview";
          case "check_desktop_release":
            return checkRelease(desktop);
          case "download_desktop_release":
            return download(desktop);
          case "cancel_desktop_download":
            if (installs[desktop].value.status !== "downloading" &&
              installs[desktop].value.status !== "ready_to_install" &&
              !(installs[desktop].value.status === "failed" && installs[desktop].value.operation === "download"))
              throw "Нет скачивания, которое можно отменить";
            stop(desktop);
            installs[desktop].publish(available(desktop));
            return;
          case "get_desktop_running_apps":
            return scenario === "confirmation"
              ? [desktop === "claude" ? "Claude" : "ChatGPT", "Codex"]
              : [];
          case "install_desktop_release": {
            if (
              installs[desktop].value.status !== "ready_to_install" &&
              !(
                installs[desktop].value.status === "failed" &&
                installs[desktop].value.operation === "install"
              )
            )
              throw "Сначала скачайте приложение";
            if (scenario === "confirmation" && (!args.confirmed || !args.close))
              throw "Подтвердите замену и закрытие приложения";
            installs[desktop].publish({
              status: "installing",
              close: Boolean(args.close),
            });
            later(() => {
              const next = { ...agents.value };
              // Installation changes the application, never its connection settings.
              const unconfigured = { routing_mode: "disabled" as const, token: null, proxy_needs_apply: false };
              if (desktop === "claude")
                next.claude_desktop = {
                  status: "found",
                  data: {
                    ...(next.claude_desktop.status === "found" ? next.claude_desktop.data : { ...unconfigured, configuration_needs_apply: false }),
                    media_needs_apply: false,
                    path: release(desktop).path,
                    version: versions[desktop],
                  },
                };
              else
                next.codex = {
                  status: "found",
                  data: {
                    ...(next.codex.status === "found" ? next.codex.data : unconfigured),
                    media_needs_apply: false,
                    cli:
                      next.codex.status === "found"
                        ? next.codex.data.cli
                        : { status: "not_found" },
                    desktop: {
                      status: "found",
                      data: {
                        path: release(desktop).path,
                        version: versions[desktop],
                      },
                    },
                  },
                };
              agents.publish(next);
              installs[desktop].publish({
                status: "installed",
                version: versions[desktop],
                notices: [],
              });
            }, 1500);
            return;
          }
          case "logout_hub_account":
            account.publish({ ...account.value, signed_in: false });
            return;
          case "login_hub_account":
            account.publish({ ...account.value, pending: true });
            later(() => {
              account.publish({
                ...account.value,
                signed_in: true,
                pending: false,
              });
              checkIdleReleases();
            }, 500);
            return;
          case "get_agent_backups":
            return scenario === "empty_backups"
              ? []
              : [
                  { id: 3, time_created: "2026-11-18T17:10:00Z", files: {} },
                  { id: 2, time_created: "2026-11-17T09:25:00Z", files: {} },
                ];
          case "apply_direct_change":
            return;
          case "prepare_agent_change": {
            const change = args.change as AgentChange;
            if ("ClaudeDesktopSetMedia" in change) {
              const agent = "claude_desktop";
              const desktop = agents.value.claude_desktop;
              const changed = mediaEnabled.has(agent) !== change.ClaudeDesktopSetMedia ||
                (desktop.status === "found" && desktop.data.media_needs_apply);
              return {
                agent, change, token: null, plan: { preview: true, agent, mediaOnly: true },
                info: changed ? [{ description: "Обновит регистрацию CoreInfra Media в Claude Desktop." }] : [],
                restarts_desktop: changed,
              } satisfies PreparedAgentChange;
            }
            const operation = Object.keys(change)[0] as keyof typeof change;
            const agent = (
              {
                CodexSetRouting: "codex",
                ClaudeSetRouting: "claude",
                ClaudeDesktopSetRouting: "claude_desktop",
                SetPiHub: "pi",
                SetOpenCodeHub: "opencode",
              } as Record<string, AgentId>
            )[operation];
            const mode = change[operation];
            const choice = args.token as TokenChoice | null;
            let resolvedToken: string | null = null;
            if (mode !== "disabled" && choice?.kind === "existing") {
              const existing = tokens.value.tokens.find((item) => item.token === choice.token);
              if (!existing) throw "Выберите токен из текущего хаба";
              resolvedToken = existing.token;
            } else if (mode !== "disabled" && choice?.kind === "create") {
              resolvedToken = `demo-${agent}-${tokens.value.tokens.length}-not-a-credential`;
              const name = {
                codex: "agent-codex",
                claude: "agent-claude-code",
                claude_desktop: "agent-claude-desktop",
                opencode: "agent-opencode",
                pi: "agent-pi",
              }[agent];
              tokens.publish({ ...tokens.value, tokens: [...tokens.value.tokens, {
                token: resolvedToken,
                name,
                issued_at: Math.floor(Date.now() / 1000),
              }] });
            }
            return {
              agent,
              change,
              token: resolvedToken,
              plan: { preview: true, agent, mode, token: resolvedToken },
              info: [
                { description: "Сохранит резервную копию настроек агента." },
                {
                  description:
                    "Обновит режим подключения и токен CoreInfra Hub.",
                },
              ],
              restarts_desktop: agent === "claude_desktop" || agent === "codex",
            } satisfies PreparedAgentChange;
          }
          case "apply_agent_plan": {
            const plan = args.plan as {
              preview: boolean;
              agent: AgentId;
              mode: RoutingMode;
              token: string | null;
              mediaOnly?: boolean;
            };
            if (!plan.preview)
              throw "Превью принимает только демонстрационные планы";
            if (plan.mediaOnly) return;
            const detection = agents.value[plan.agent];
            if (detection.status === "found")
              agents.publish({
                ...agents.value,
                [plan.agent]: {
                  ...detection,
                  data: { ...detection.data, routing_mode: plan.mode, token: plan.token, proxy_needs_apply: false, ...(plan.agent === "claude_desktop" && { configuration_needs_apply: false }), ...(plan.agent === "claude" && { media_needs_apply: false }) },
                },
              });
            return;
          }
          case "set_application_setting": {
            if (scenario === "settings_error")
              throw "Не удалось сохранить настройку. Попробуйте ещё раз.";
            application.publish({
              ...application.value,
              [args.setting as string]: Boolean(args.enabled),
            });
            return;
          }
          case "set_system_proxy": {
            if (systemProxy.value.conflict && !args.forced)
              throw "Обнаружены другие настройки прокси";
            systemProxy.publish({ ...systemProxy.value, busy: true });
            return new Promise<void>((resolve) =>
              later(() => {
                systemProxy.publish({
                  ...systemProxy.value,
                  status: args.enabled ? "on" : "off",
                  busy: false,
                  conflict: false,
                });
                resolve();
              }, 400),
            );
          }
          case "select_hub":
            account.publish({
              ...account.value,
              active_hub_id: String(args.hubId),
            });
            tokens.publish({ ...tokens.value, hub_id: String(args.hubId) });
            summary.publish({ ...summary.value, hub_id: String(args.hubId) });
            return;
          case "open_hub":
            openedHubPages.push({ hubId: account.value.active_hub_id, section: String(args.section ?? "home") });
            if (args.section === "payments") {
              later(() => {
                summary.publish({ ...summary.value, balance_rub: 1500 });
                window.dispatchEvent(new Event("focus"));
              }, 600);
            }
            return;
          case "read_logs":
            return `[2026-11-18 20:10:00] INFO ${args.source === "proxy" ? "Proxy" : "Wizard"}: ready\n[2026-11-18 20:10:01] INFO Background release check completed\n`;
          case "request_update":
            downloadWizardUpdate();
            return;
          case "cancel_update":
            if (update.value.status === "downloading" || update.value.status === "ready_to_install") {
              updateGeneration++;
              update.publish({ status: "available", version: "1.0.6" });
            }
            return;
          case "install_update":
            if (update.value.status !== "ready_to_install") return;
            update.publish({ status: "installing", version: "1.0.6" });
            later(() => update.publish({ status: "up_to_date" }), 1000);
            return;
          case "ensure_agent_proxy":
            proxy.publish({ enabled: true });
            return;
          case "refresh_hub_tokens":
          case "launch_agent":
          case "open_hub_registration":
          case "open_logs":
            return;
          // All IPC stays local. Unknown actions fail visibly instead of reaching a real service.
          default:
            if (
              command.startsWith("plugin:log|") ||
              command === "plugin:window|set_theme"
            )
              return;
            throw `Действие «${command}» не входит в превью установки.`;
        }
      },
      { shouldMockEvents: true },
    );
    checkIdleReleases();
    if (scenario === "checking" || scenario === "startup")
      later(() => update.publish({ status: "up_to_date" }), 4000);
    if (scenario === "downloads") {
      download("claude", 38);
      download("chatgpt", 64);
    }
    if (scenario === "mixed_progress") download("chatgpt", 38);
  }

  function dispose() {
    disposed = true;
    stop("claude");
    stop("chatgpt");
    timeouts.forEach(clearTimeout);
    timeouts.clear();
    for (const store of [
      agents,
      account,
      proxy,
      update,
      tokens,
      application,
      systemProxy,
      configured,
      installs.claude,
      installs.chatgpt,
    ])
      store.waiting.clear();
  }
  return {
    start,
    dispose,
    finishDownloads,
    setDownloadSpeed,
    calls,
    update,
    openedHubPages,
    agents,
    tokens,
    account,
    summary,
    installs,
    application,
    systemProxy,
  };
}

let activeEnvironment: ReturnType<typeof createMockEnvironment> | undefined;
export function startPreviewMocks(selectedScenario: Scenario = scenario, options: MockOptions = {}) {
  activeEnvironment?.dispose();
  activeEnvironment = createMockEnvironment(selectedScenario, options);
  activeEnvironment.start();
  return activeEnvironment;
}
export function finishDownloads() {
  activeEnvironment?.finishDownloads();
}
export function setDownloadSpeed(value: number) {
  activeEnvironment?.setDownloadSpeed(value);
}

export function showOverviewTransition() {
  const agents = activeEnvironment?.agents;
  if (!agents || agents.value.claude_desktop.status !== "found") return;
  agents.publish({ ...agents.value, claude_desktop: {
    ...agents.value.claude_desktop,
    data: { ...agents.value.claude_desktop.data, routing_mode: "hub" },
  } });
}
