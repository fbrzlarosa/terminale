# Uninstalling

`terminale` keeps to standard, per-OS locations, so removing it is
straightforward. There are two parts: the **program** itself and your
**user data** (config, themes, plugins). Removing the program leaves your data
untouched unless you delete it explicitly.

## Remove the program

### Windows

- **Installed via the `.msi`:** Settings → Apps → Installed apps → *terminale* →
  Uninstall. Or from an elevated PowerShell:

  ```powershell
  winget uninstall terminale
  ```

- **Installed via the PowerShell one-liner:** run the bundled uninstaller, or
  delete the install directory it reported (typically under
  `%LOCALAPPDATA%\terminale\`).

### macOS

- **Installed via the `.pkg`:** delete the app bundle:

  ```bash
  rm -rf /Applications/terminale.app
  ```

- **Installed via Homebrew:**

  ```bash
  brew uninstall terminale
  ```

### Linux

- **Installed from the tarball:** delete the binary you placed on your `PATH`:

  ```bash
  rm -f ~/.local/bin/terminale   # or wherever you installed it
  ```

- **Installed via Homebrew (Linuxbrew):**

  ```bash
  brew uninstall terminale
  ```

### Built from source

```bash
cargo uninstall terminale     # if you used `cargo install`
# otherwise just delete the checkout and ./target
```

## Remove user data (optional)

Your settings, themes, and plugins live in the per-OS config directory. Delete it
only if you want a completely clean removal.

| OS | Config / data directory |
|---|---|
| Linux | `$XDG_CONFIG_HOME/terminale/` (fallback `~/.config/terminale/`) |
| macOS | `~/Library/Application Support/terminale/` |
| Windows | `%APPDATA%\terminale\` |

That directory contains `config.toml` and the `themes/` and `plugins/`
subdirectories. Removing it:

```bash
# Linux
rm -rf ~/.config/terminale

# macOS
rm -rf ~/Library/Application\ Support/terminale
```

```powershell
# Windows
Remove-Item -Recurse -Force "$env:APPDATA\terminale"
```

There is **no global system state, service, registry hive, or daemon** to clean
up beyond the program and that config directory. If you registered a global
Quake hotkey through the OS (rather than in-app), unregister it the same way you
added it.
