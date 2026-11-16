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

### Overview and initial setup

**Обзор** is the default full-window page, above **Агенты** in the sidebar.
The three action cards use the live account and detection state:

- **Подключите свой аккаунт** becomes the selected hub and account identity after sign-in.
- **Установите агентов** becomes available after sign-in.
- **Настройте агентов** requires sign-in and at least one detected agent.

On first launch, navigation is hidden. «Пропустить быструю настройку» beside
«Начало работы» reveals it and remembers the choice across launches. Overview
remains accessible from the sidebar. Configuration reuses the regular agent form
and plan confirmation; multiple installed agents always show a selection first.
The final action launches the configured desktop app or CLI.

Connection options are ordered API access, subscription, then without CoreInfra.
The default token is selected automatically: reuse the configured token if it belongs to the
selected Hub, otherwise reuse the oldest token named for the agent, or create one
on confirmation. The note's «Изменить» link opens a menu of tokens in the selected
Hub. The choice is shared by API configuration and downloads. Claude Desktop's
subscription uses Wizard's shared proxy token and needs no separate agent token.
The local proxy has exactly two listeners: `127.0.0.1:33787` for Claude Desktop and
the system proxy, and `127.0.0.1:33788` for ChatGPT. Both use the same Wizard token;
changing an agent's API token does not change either listener's credential.
«Завершить настройку» opens the change plan before the launch step. Regular agent
pages use «Применить», enabled for pending mode/token changes or a stopped proxy.

Downloads run in the backend and preserve explicit pauses across dialog closes.
Installing requires a separate «Установить» confirmation; «Отмена» retains the
archive. Installed apps show their version and offer an update check. Completed
installs and checks close with «Готово».

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
