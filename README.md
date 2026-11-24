# CoreInfra Wizard

An application for configuring agents to use CoreInfra.

## Media in ChatGPT Work, Codex and Claude Code

Enable **Подключить `coreinfra-media` для видео-моделей** under **Дополнительные модели** on the agent’s page and apply the change. Ask for a video in a new ChatGPT Work or Codex chat, or invoke `$coreinfra-video`.

The bundled skill and the `coreinfra_media` MCP registration live in the profile selected by `CODEX_HOME`. The Wizard user daemon listens at `http://127.0.0.1:33789/mcp` and uses a separate local bearer for each enabled agent. Turning the switch off on the ChatGPT / Codex page revokes local access and removes the managed registration and unmodified skill files. The opt-out persists across restarts.

Upgrading from Wizard 1.0.10: existing `coreinfra_video` registrations, skills and local grants are migrated to `coreinfra_media` on the next **Применить**, preserving Codex server restrictions, Claude Desktop `toolPolicy` rules and opt-out state. The legacy `--video-mcp-stdio` flag is still accepted. If you added custom `mcp__coreinfra_video__*` permission rules in Claude Code settings, update them to `mcp__coreinfra_media__*`.

Each paid request uses the calling agent’s active CoreInfra key, cached in memory on startup and after applying settings. Media tools do not issue keys or use the selected Wizard account. Wizard keeps no job table or MCP session. All files are written to **Downloads/CoreInfra Media**; callers cannot choose the path. Repeating a paid submission after a network timeout can incur another charge, so the tools never retry it automatically.

### Video

`video_models` returns the public catalog and prices in RUB per second. `video_create` submits one paid job. Save the full returned `job` object and pass it to `video_result` to poll, including after restarting Wizard. The original Hub and key must still be available. A completed download returns a local MP4 in **Downloads/CoreInfra Media**. Use `download: false` to poll without creating a file. fal also returns its signed result URL in this mode. A job records the key fingerprint, provider, model and upstream ID; polling with a different key is rejected.

### Claude Code

In Wizard choose **Claude Code → API-доступ → Применить** and confirm the configuration plan. Start a new local Claude Code session and ask «Какие видеомодели доступны?» or invoke `/coreinfra-video`. Wizard can be closed after configuration. The CLI routing command applies the same configuration.

Wizard installs the bundled skill under `~/.claude/skills/coreinfra-video` and registers the HTTP MCP server `coreinfra_media` in the user’s `~/.claude.json`. Both locations follow `CLAUDE_CONFIG_DIR` when set. Claude Code reads the current local bearer through its native `headersHelper`. Enable the media switch separately for Claude Code; the registration stays active across routing modes while enabled. Disabling the media switch revokes its bearer and removes its headers file, managed MCP entry and unmodified skills without affecting Codex or Claude Desktop.

### Claude Desktop

Claude Desktop uses the `coreinfra_media` connector configured by Wizard together with the plugin `coreinfra-media` from the marketplace `coreinfra-wizard` (this repository’s `.claude-plugin/marketplace.json`).

The Claude Code E2E in the Linux, macOS and Windows CI matrix configures a fresh profile, launches native Claude Code, asks it to read the installed skill, and verifies that the next inference request and the final answer receive live catalog data. Chat inference is a local Messages fixture. To run it locally, set `WIZARD_VIDEO_TEST_CLAUDE` to a Claude Code executable and run `cargo test -p wizard-core claude_code_skill_lists_public_media_models -- --ignored --nocapture` with Python 3 available.

### Custom fields

Put provider-specific JSON in `fields`. Wizard merges its entries into the top level of the outgoing JSON body and preserves nested objects, arrays, numbers, booleans and nulls. For clients that cannot express open-ended object arguments, `fields_json` accepts the same object as a JSON string. Use one representation per request. Named tool arguments (such as `model`, `prompt`, `input`, `voice`) are reserved and must not be repeated inside `fields`. The destination, authentication and HTTP headers are controlled by Wizard.

```json
{
  "model": "blackforestlabs/flux-3-max/image-to-video",
  "prompt": "The paper boat drifts downstream",
  "fields": {
    "image_url": "data:image/png;base64,<encoded first frame>",
    "duration": 5,
    "seed": 0,
    "generate_audio": false
  }
}
```

For OpenRouter, put per-provider options under `fields.provider.options` and inline frame images under `fields.frame_images`. The Hub strips other OpenRouter routing options and `callback_url`. fal accepts inline `image_url` and `end_image_url` and uses polling for job status. The MCP request body limit is 32 MiB including base64 encoding. The Hub and model determine which fields they honor. [OpenRouter media](https://coreinfra.ru/docs/use/openrouter) · [fal queue](https://coreinfra.ru/docs/use/fal)

The [client compatibility test](crates/wizard-core/tests/media_clients.py) includes a Python SDK 2.x example using `streamable_http_client`. To repeat the end-to-end test, set `WIZARD_VIDEO_TEST_PYTHON` to a Python with `mcp` 2.x installed and `WIZARD_VIDEO_TEST_CODEX` to a Codex executable. Run `cargo test -p wizard-core native_and_python_clients -- --ignored --nocapture`. All submissions in this test go to its temporary local Hub.

CI also runs a catalog E2E on Linux, macOS and Windows. Wizard installs the bundled skill and MCP registration into a fresh profile. A native Codex turn uses the installed skill to call `video_models` and receives model IDs from the public Hub. A local Responses fixture requests that single read-only tool call. To run it locally, set `WIZARD_VIDEO_TEST_CODEX` to a Codex executable or its npm `bin/codex.js` launcher and run `cargo test -p wizard-core codex_skill_lists_public_media_models -- --ignored --nocapture` with Python 3 available. The test uses its own loopback listener.

## How to build

1. Install Rust and Node.js:

   - [Rust via rustup](https://rustup.rs/). The required Rust version will be downloaded automatically when you build the project.
   - [Node.js 24](https://nodejs.org/) with npm.

2. Install the Tauri prerequisites for your operating system:

   - **macOS:** install Xcode Command Line Tools with `xcode-select --install`.
   - **Windows:** install Visual Studio Build Tools with the **Desktop development with C++** workload and the WebView2 Runtime.
   - **Linux:** follow the [instructions for your distribution](https://v2.tauri.app/start/prerequisites/#linux).

3. Clone the repository:

   ```sh
   git clone https://github.com/CoreInfraAI/wizard.git
   cd wizard
   ```

4. Install dependencies:

   ```sh
   npm ci --include=dev --prefix crates/wizard-gui/wizard-ui
   ```

5. Choose a bundle format for your system:

   - **macOS:** `app` (application), `dmg` (disk image).
   - **Windows:** `nsis` (EXE installer), `msi` (MSI installer).
   - **Linux:** `appimage` (AppImage), `deb` (Debian / Ubuntu), `rpm` (Fedora / RPM).

   Build from the repository root, replacing `BUNDLE` with your chosen format (for example, `app` on macOS):

   ```sh
   node crates/wizard-gui/wizard-ui/node_modules/@tauri-apps/cli/tauri.js build --bundles BUNDLE
   ```

   On macOS, a Finder window showing the mounted disk image may open while the `.dmg` is being built. Don't interact with that window.

   If DMG creation fails, build with `--bundles app` instead or allow your terminal to control Finder in **System Settings → Privacy & Security → Automation**.

The generated files are located at these paths relative to the repository root:

| Bundle | Path |
| --- | --- |
| `app` | `target/release/bundle/macos/CoreInfra Wizard.app` |
| `dmg` | `target/release/bundle/dmg/*.dmg` |
| `nsis` | `target/release/bundle/nsis/*.exe` |
| `msi` | `target/release/bundle/msi/*.msi` |
| `appimage` | `target/release/bundle/appimage/*.AppImage` |
| `deb` | `target/release/bundle/deb/*.deb` |
| `rpm` | `target/release/bundle/rpm/*.rpm` |

`*` represents the filename, which depends on the application version and architecture.

### Working on the interface on macOS

Run these commands from the repository root. Quit any running Wizard first: the
application allows only one instance, including development builds.

For UI development with Vite hot reload and Rust file watching:

```sh
node crates/wizard-gui/wizard-ui/node_modules/@tauri-apps/cli/tauri.js dev
```

For a standalone local application (a faster debug build, signed ad hoc):

```sh
node crates/wizard-gui/wizard-ui/node_modules/@tauri-apps/cli/tauri.js build --debug --bundles app --config '{"plugins":{"updater":{"endpoints":[]}}}'
open "target/debug/bundle/macos/CoreInfra Wizard.app"
```

The empty updater endpoint list disables update checks in this local bundle, so a
published release cannot replace the code you are testing.

Omit `--debug` for the release build described above. Neither command installs the
bundle into `/Applications`. Local builds use the same saved account and proxy
settings as the installed Wizard. Quit and reopen the bundle after rebuilding it.

The interface reference is the Hub in the neighboring `thinker` repository:
`crates/ff-hub/hub-ui/src/modules/App.tsx` (agent order and divider) and
`packages/coreinfra-ui/src/hub-sidebar-footer.tsx` (account menu and theme controls).

### UI design system

Wizard uses neutral Catalyst surfaces, shared typography and spacing, and
restrained state colors. Startup, overview drawing, and download progress
animations remain part of the interface. Navigation and dialog transitions must
not reset ongoing operations.

`src/design-system/tokens.css` owns surface, text, border, spacing, radius, and
motion values. `src/components/ui` owns page headings, panels, action bars,
identities, notices and transfer cards; accessible controls and focus trapping
continue to use Catalyst / Headless UI. Connection forms use unframed panels
inside dialogs. Version rows share column alignment; on narrow screens their
actions move below the version and stay together.

Body/control text is 14/24 px, metadata 12/20 px, section titles 16/24 px and page
titles 22/32 px. Text inputs retain 16 px below 640 px to avoid mobile auto-zoom.
Do not use page-wide `!important` typography overrides or make a portal inherit
typography from the page behind it. Preserve heading order, keyboard focus,
contrast in both themes and reduced-motion support. Download progress uses a
labeled progressbar instead of announcing every percentage tick.

Run the application checks from the repository root:

```sh
npm test --prefix crates/wizard-gui/wizard-ui
npm run build --prefix crates/wizard-gui/wizard-ui
```

The flow tests cover background downloads, cancellation, explicit installation,
version changes, first launch, connection plans and Hub balance states. Browser
mocks do not exercise native archive installation or OS proxy changes.

### Storybook

```sh
npm run storybook --prefix crates/wizard-gui/wizard-ui
npm run build-storybook --prefix crates/wizard-gui/wizard-ui
```

Open <http://127.0.0.1:6006/>. The catalog contains design tokens, Catalyst controls,
reusable product blocks, actual application screens and interaction flows. It
includes Controls, component docs, accessibility checks, isolated light/dark
themes, reduced motion, and 360/640/800/1100/1440 px viewports. The static catalog
builds to `wizard-ui/storybook-static`; CI runs the frontend tests and builds both
the application and catalog. The Catalyst license remains attached to its sources.

`src/test/stories.test.tsx` runs the stories' actual play functions. The virtualized
Combobox story is browser-only because JSDOM has no layout; verify it in the
catalog. Check geometry, contrast, overflow and focus at the catalog's viewports.

Useful stories:

- `overview-transition--installation-into-control-panel` shows the installation
  card folding into the management button, with replay/reset controls.
- `сценарии-переходы--agent-update` exercises download, installation and version change.
- `сценарии-переходы--setup-configuration` opens connection settings from management.
- `сценарии-переходы--subscription-key` selects and saves a demo key across API and subscription modes.
- `сценарии-переходы--narrow-activity` opens a download from the narrow sidebar menu.

The scoped override of `@joshwooding/vite-plugin-react-docgen-typescript` to 0.9.1
avoids the vulnerable glob/minimatch chain selected by Storybook 10.6.7's older
range. `.storybook/main.ts` skips virtual story-index HMR events for this plugin,
which otherwise reads null-prefixed module IDs as files. Remove the override and
guard when upstream supports the updated dependency and filters virtual updates.

### Overview and initial setup

**Обзор** is the default full-window page, above **Агенты** in the sidebar.
The three action cards use the live account and detection state:

- **Подключите свой аккаунт** becomes the selected hub and account identity after sign-in.
- **Установите агентов** becomes available after sign-in.
- **Настройте агентов** requires sign-in and at least one detected agent.

First launch shows a brief Wizard logo animation, then automatically opens the
full interface with navigation, even before sign-in or setup. The animation is
remembered across launches and skipped for reduced motion or completed legacy
onboarding. Update checks run in the background without blocking navigation.
Configuration reuses the regular agent form and plan confirmation; multiple
installed agents always show a selection first. The final action launches the
configured desktop app or CLI.

Connection options are ordered API access, subscription, then without CoreInfra.
The default token is selected automatically: reuse the configured token if it belongs to the
selected Hub, otherwise reuse the oldest token named for the agent, or create one
on confirmation. The note's «Изменить» link opens a menu of tokens in the selected
Hub. This choice applies to the agent's API configuration. Desktop update checks
and downloads use Wizard's own `coreinfra-ai-wizard` token from the current
account's personal Hub, reusing it or creating it when missing. Claude Desktop's
subscription uses the same shared proxy token and needs no separate agent token.
The local proxy has exactly two listeners: `127.0.0.1:33787` for Claude Desktop and
the system proxy, and `127.0.0.1:33788` for ChatGPT. Both use the same Wizard token;
changing an agent's API token does not change either listener's credential.
«Завершить настройку» opens the change plan before the launch step. Regular agent
pages use «Применить», enabled for pending mode/token changes or a stopped proxy.

Downloads run in the backend independently of the current page or dialog. The
action beside an agent's installed version is «Обновить» («Скачать» for a missing
desktop app). It only opens that agent's installation panel. Download, install
and cancel actions live in the expanded panel; the picker also only opens it.
The version row keeps the installed version until installation completes, then
animates the new number and removes the update action. Release checks run
automatically, without a manual check button or an up-to-date message.
Closing the dialog (outside click, Escape, or cross) keeps downloads running in
a stack of compact widgets in the left sidebar, directly above the Hub divider,
with a logo, percentage and downloaded/total MB. Clicking
a widget restores that operation. «Назад» appears only after choosing an app in
the picker and returns there without stopping downloads. Direct openings from
a version action or sidebar widget have no Back button. «Отменить» stops the selected
transfer and deletes its cached archive; downloading again starts from zero.
Network interruptions still retain resumable progress. Installation always needs
a separate confirmation, including notices about closing running applications.

Wizard itself also checks for updates automatically, including at launch. Its
sidebar update action opens a dialog without changing the installed application.
«Скачать обновление» downloads and verifies the signed archive; «Установить» then
replaces Wizard and restarts it. Downloads survive closing the dialog and remain
visible in the sidebar. «Отменить» aborts the download and discards its bytes.
The main interface stays mounted throughout; only the explicit installation step
prepares the proxy service for replacement.

### Browser preview with mocks

Run from the repository root:

```sh
npm run dev:mock --prefix crates/wizard-gui/wizard-ui
```

Open <http://127.0.0.1:1422/preview.html>. This uses the actual application
components with local Tauri IPC mocks. It does not install applications, download
archives, or contact Hub. The separate preview entry is excluded from the desktop
production bundle. Reloading the preview resets all simulated operations.

The «UI-превью · Моки» panel switches scenarios: first launch, background checks,
failed checks, available update, missing agents,
current version, downloaded archive, failed download with retry, reconnection,
unknown size, required confirmation/running apps, signed-out account, and an
unsupported platform. It also changes download speed, finishes downloads, resets
the scene, and switches themes. A normal simulated download takes 36 seconds.
For example, `preview.html?scenario=missing` opens the first-install scenario.
`preview.html?scenario=downloads` starts two slower downloads to inspect the sidebar
stack above the Hub divider in light and dark themes.
`mixed_ready`, `mixed_progress` and `mixed_error` combine different states in
neighboring cards to check alignment and independent actions.
`all_agents` shows the daily overview with two illustrated cards: Hub balance and
agent management. Its only action, «Панель управления», opens the list of agents.
Each update opens its installation panel. Clicking an agent's connection status
opens its settings inside management; «Назад» discards the draft and returns to
the list. There are no generic install/setup buttons in management.
The agent summary shows two names with logos and an overflow count. Drawings on
enabled overview cards animate and turn blue on hover or keyboard focus, both
during onboarding and after setup. The marker introduction is limited to
onboarding; reduced motion keeps the color feedback without movement. The video
banner is shared. Installation and update panels separate their actions with
whitespace, without a horizontal rule.
`unconfigured` shows the first connection flow. `onboarding_merge` demonstrates
the installation card folding into the «Панель управления» button after
setup finishes. The mock toolbar's «Показать переход» also plays it directly.
Reduced-motion preferences skip the fold. `wizard_update` exercises the separate
Wizard download/install dialog with simulated progress and no actual replacement.
`balance_empty`, `balance_guest`, and `balance_error` cover the top-up prompt,
guest access and unavailable balance. Guests see the Hub identity and its action,
without a balance block or a replacement access label. In the mock preview, «Пополнить» simulates
returning from Hub with 1,500 ₽; no real payment occurs. The native application
reads `/hub/api/hub/info` and opens `/payments` for the selected Hub. Balance
refreshes on window focus and every 30 seconds; missing data never means zero.
Agent pages always show the connection form, aligned with the page content and
without an outer frame, collapsed summary or cancel action. «Применить» stays on
the right and is enabled only for pending changes; after saving, the form stays
open with the saved values and the action becomes disabled. Missing CLI agents
offer a copyable installation command. Connection modes share one framed radio
group. Connected agents show «CoreInfra AI Hub» as their status.
The mock preview offers two demo keys in both «API-доступ» and «Своя подписка»;
the selected key persists when switching modes and is saved with the mock plan.
This subscription key selection is preview-only: native Claude Desktop still
uses the shared Wizard proxy key. No proxy authorization changes are simulated
outside the mock environment.
Settings have their own navigation item. Downloads appear only at the bottom of
the left sidebar, above Hub; below 1024 px, open the navigation menu to access
the same widgets. There is no download indicator in the top bar.
Each app keeps its own progress indicator, so completing one transfer does not
change the other's percentage. `chatgpt_error` shows a nested detection failure
with details inside management; `unconfigured` also covers a stopped, unused proxy.
`preview.html?scenario=startup` replays the first-launch animation on each reload,
then opens Overview with all navigation and a signed-out account. Other scenarios
retain the startup preference. `preview.html?scenario=checking` keeps the interface
usable while releases are checked automatically and appear after three seconds.

To verify the interaction: open the installation picker from onboarding, choose
an app, then download from its expanded panel. Choose «Назад», select the other
app, start its download, then close the dialog. Both widgets should keep progressing.
Reopen one and cancel: only that download resets. Finish the other from the mock
panel and verify that installation starts only after clicking its confirmation.

Choosing a subscription offers to enable the system proxy if it is off.
Only «Включить» changes OS settings; cancel keeps the selected mode. The prompt
does not overwrite another proxy. Disable it in Settings → System proxy.

The [Wizard demo](https://coreinfra.ru/wizard) loads only after a click; its poster
is local. `coreinfra.ru/assets/wizard/` is allowed in both Tauri media CSPs.
Controls use Catalyst with 14 px text. Card surfaces stay neutral; drawings gain
color on hover/focus. The first account-card hover plays the marker animation
once per launch; newly unlocked steps circle only their buttons. Animations
respect Reduced Motion.

## License

Except where separately noted, CoreInfra Wizard's original source code is licensed under the [Mozilla Public License, v. 2.0](LICENSE). If a copy of the MPL was not distributed with this project, you can obtain one at <https://mozilla.org/MPL/2.0/>.

The Catalyst components and their derivatives in [`crates/wizard-gui/wizard-ui/src/components/catalyst/`](crates/wizard-gui/wizard-ui/src/components/catalyst/) are governed by the [Tailwind Plus commercial license](crates/wizard-gui/wizard-ui/src/components/catalyst/LICENSE.md) and are excluded from the MPL-2.0 grant, including when bundled in the application. Other third-party materials retain their respective licenses.

The MPL-2.0 grant for CoreInfra Wizard's original code applies to all versions, including those released before the license file was added. This statement does not extend to separately licensed third-party materials.
