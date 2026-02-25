# Shift

> **A GUI-first replacement for TTYs.**

**Shift** is a session manager that replaces the traditional TTY-based session model with a fully graphical mode session manager that manages multiple compositor sessions, input routing, and transitions between them.



**Clients:**
- [ardos-wm](https://github.com/ardos/ardos-wm): Ardos OS's wayland compositor, forked from [Hyprland](https://github.com/hyprwm/Hyprland) | Session Client
- [tibs](https://github.com/ardos/tibs) *(Planned)*: Ardos OS's login screen and boot splash | Admin Client
## Key Features

* **GPU-accelerated animations**: Switching between 2 users is just switching the OpenGL texture it is rendering to the screen, allowing for more interesting transitions
* **GUI-first design** — Shift takes over your screen as soon as your init system starts so you never see a TTY or an fbconsole
* **Input routing** - Shift mediates the input events between the compositor and libinput, removing the need for libseat
* **Token protected IPC** - Compositors must have a token provided by the Admin Client (usually the display manager) to connect to Shift. Tokens are single-use and are consumed once the compositor connects preventing random processes from connecting and causing chaos.
* **Safer session switching** - You can't <kbd>CTRL</kbd> + <kbd>ALT</kbd> + <kbd>F\<N\></kbd> your way to another user's session, only the display manager is able to switch your session.

## Admin Client vs Session Client

The admin client is the client that starts up and manages other compositors processes, it is usually the display manager/login screen. The admin client has special permissions such as creating new tokens/sessions and switching the current session.
Shift requires a path to the admin client binary to be passed in `SHIFT_ADMIN_CLIENT_BIN` environment variable. It then, right after binding to the unix socket at `/tmp/shift.sock`, executes the admin client binary passing a admin token in the `SHIFT_SESSION_TOKEN` environment variable.

When the admin creates new tokens, it usually creates sessions with a `Session`/`Normal` role, which means they're unpriviliged.

## 🚧 Status

- [X] Define the protocol
- [X] Receive connections from clients
- [X] Authentication
- [X] Receive frame commits and render them on the screen (with vsync)
- [X] Session switching
- [ ] Allow disabling vsync
- [X] Inputs
- [ ] Audio isolation
