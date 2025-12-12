# Shift

> **A GUI-first replacement for TTYs.**

**Shift** is a next-generation display session orchestrator that manages multiple compositors and graphical environments with seamless, GPU-driven transitions.
It serves as the visual and structural core of the new Hyprside session model — replacing traditional text-based TTYs with a fluid, interactive, and secure graphical layer that finally brings Linux to the GUI era the way it should've been.

---

## ✨ Overview

Shift coordinates multiple display servers (compositors) running in isolation.
Only one session is visible at a time, ensuring focus and security, while others remain suspended or in background.
Thanks to hardware-accelerated composition, transitions between sessions are instantaneous and visually fluid — enabling smooth logins, user switching, and crash recovery animations.

---

## 🧠 Key Features

* **Multiple isolated sessions** — each compositor runs in its own sandboxed process.
* **GPU-driven transitions** — seamless visual handoffs between active sessions such as when switching users or logging in and out.
* **GUI-first design** — no text TTYs; everything is graphical and interactive.
* **Crash recovery layer** — automatically returns to the main environment if the current session disconnects abruptly.
* **Secure token-based IPC** — each session connects through one-time authentication tokens.
* **Input routing** — only the active session receives input; background sessions remain paused.
* **VSync and cursor planes** — precise frame synchronization and hardware cursor control.

---

## 🪄 Philosophy

Traditional Linux systems rely on text-based TTYs and display managers that abruptly start or stop sessions a lot of the times revealing the FB console behind and black screens.
Shift reimagines this workflow: instead of killing and respawning environments, it manages multiple display server connections, allowing to smoothly switch between them with transitions, like they were just different virtual workspaces.

This approach allows:

* Instant user switching
* Better experience when the desktop environment crashes
* Persistent and continuous graphical experience from boot to shutdown.

---

## ⚙️ Integration

Shift is agnostic to its clients ([Read the protocol here](tab/v1.md)) and can be integrated with any compositor or graphical shell.
For Hyprside, it powers the entire session lifecycle:

* **TIBS** (boot & session shell) acts as the administrator client.
* **HyprDE** and other compositors connect as standard clients.
* Each connection spawns a managed session, isolated yet fully synchronized with GPU fences and input streams.

---

## 🚧 Status

- [X] Define the protocol
- [X] Receive connections from clients
- [X] Authentication
- [X] Receive frame commits and render them on the screen (with vsync)
- [X] Session switching
- [ ] Allow disabling vsync
- [X] Inputs
- [ ] Audio isolation
