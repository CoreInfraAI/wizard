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
node crates/wizard-gui/wizard-ui/node_modules/@tauri-apps/cli/tauri.js build --debug --bundles app
open "target/debug/bundle/macos/CoreInfra Wizard.app"
```

Omit `--debug` for the release build described above. Neither command installs the
bundle into `/Applications`. Local builds use the same saved account and proxy
settings as the installed Wizard. Quit and reopen the bundle after rebuilding it.

The interface reference is the Hub in the neighboring `thinker` repository:
`crates/ff-hub/hub-ui/src/modules/App.tsx` (agent order and divider) and
`packages/coreinfra-ui/src/hub-sidebar-footer.tsx` (account menu and theme controls).

## License

Except where separately noted, CoreInfra Wizard's original source code is licensed under the [Mozilla Public License, v. 2.0](LICENSE). If a copy of the MPL was not distributed with this project, you can obtain one at <https://mozilla.org/MPL/2.0/>.

The Catalyst components and their derivatives in [`crates/wizard-gui/wizard-ui/src/components/catalyst/`](crates/wizard-gui/wizard-ui/src/components/catalyst/) are governed by the [Tailwind Plus commercial license](crates/wizard-gui/wizard-ui/src/components/catalyst/LICENSE.md) and are excluded from the MPL-2.0 grant, including when bundled in the application. Other third-party materials retain their respective licenses.

The MPL-2.0 grant for CoreInfra Wizard's original code applies to all versions, including those released before the license file was added. This statement does not extend to separately licensed third-party materials.
