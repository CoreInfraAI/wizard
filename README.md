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
   npm ci --prefix crates/wizard-gui/wizard-ui
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
