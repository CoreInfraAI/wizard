# CoreInfra Wizard

An application for configuring agents to use CoreInfra.

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
