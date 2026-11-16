# Airbend Omarchy plugin

This is a native Omarchy bar widget (`airbend.panel`), placed in the tray drawer by default.
It runs within Omarchy's existing shell and uses the installed `airbendctl` command to read
local daemon status and select Quiet or Performance. It needs socket access through the
daemon's configured group (`wheel` by default); it never uses sudo or touches hardware.

After installing Airbend, run `airbend-omarchy-install` as your normal user. This validates
the plugin, links the package-owned files into your user plugin directory, backs up
`shell.json`, and enables the widget. Package upgrades update the linked plugin files.
After upgrading an already-running widget, use `omarchy restart shell` to refresh its
QML and JavaScript caches. A normal first installation needs no shell restart.

The `airbend` Arch package contains the daemon, desktop app, CLI, and this plugin. This
plugin's installer only registers/enables the user widget; installing the plugin by itself
does not install those binaries or start `airbendd`. Follow the repository's Arch/Omarchy
installation instructions first.

Click the fan icon to open the panel; right-click opens or focuses Airbend. Use the profile
buttons or Up/Down and Enter; Escape dismisses the panel. Open Airbend remains available
when the daemon is disconnected. Fan control continues when the desktop window is closed.

The entry's `socketPath` and `refreshIntervalSec` settings can be changed through Omarchy's
widget settings or `omarchy bar set`. The default refresh interval is three seconds; actions
trigger an immediate refresh. Subprocess calls have timeouts and use argv arrays rather than
shell interpolation. External text is bounded and rendered literally; shared tooltip/button
text is neutralized before passing it to Omarchy components.

Useful IPC calls:

```sh
omarchy-shell airbend.panel status
omarchy-shell airbend.panel show
omarchy-shell airbend.panel profile performance
omarchy-shell airbend.panel profile quiet
omarchy-shell airbend.panel hide
```

Validate and test from the checkout:

```sh
omarchy plugin validate dist/omarchy
node --test dist/omarchy/tests/model.test.cjs
```
