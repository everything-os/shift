# tab-app-framework

`tab-app-framework` is a Rust application framework for building standalone apps that run on the Tab protocol.

It gives you a GLFW-like developer experience:
- initialize an app
- receive render/input/session callbacks
- render with OpenGL
- handle monitors and cursor layout
- create and switch sessions from app code

## Crates

- `tab-app-framework`:
  Convenience re-export crate. Most apps should depend on this.
- `tab-app-framework-core`:
  Core runtime and callback/event model.
- `tab-app-framework-gl`:
  OpenGL integration and render-target setup.
- `tab-app-framework-xkb`:
  Keyboard composition helpers.
- `monitor-layout-engine`:
  Monitor layout and cursor movement utilities.

## Add to your project

```toml
[dependencies]
tab-app-framework = { path = "../app-framework" }
```

## Quick start

```rust
use tab_app_framework::{
    Config, GlApplication, GlEventContext, GlInitContext, GlTabAppFramework, RenderEvent, RenderMode,
};

struct App;

impl GlApplication for App {
    fn init(_ctx: &mut GlInitContext) -> anyhow::Result<Self> {
        Ok(Self)
    }

    fn on_render(&mut self, ctx: &mut GlEventContext<'_, '_, Self>, _ev: RenderEvent) {
        let gl = ctx.gl().glow();
        unsafe {
            gl.clear_color(0.1, 0.1, 0.1, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut app = GlTabAppFramework::<App>::init(|config: &mut Config| {
        config.opengl_version(3, 3);
        config.set_render_mode(RenderMode::Eager);
    })?;
    app.run()
}
```

## Runtime configuration

The framework expects `SHIFT_SESSION_TOKEN` in the environment by default.

You can customize:
- socket path (`Config::set_socket_path`)
- render node (`Config::set_render_node_path`)
- OpenGL version (`Config::opengl_version`)
- render mode (`Config::set_render_mode`)

## Event model

The framework uses callback methods on `GlApplication` / `Application`.

Common callbacks:
- lifecycle:
  `on_render`, `on_present`, `on_error`
- monitor:
  `on_monitor_added`, `on_monitor_removed`
- session:
  `on_session_state`
- keyboard/text:
  `on_key`, `on_char`
- pointer/mouse:
  `on_pointer_move`, `on_mouse_move`, `on_pointer_down`, `on_pointer_up`, `on_mouse_down`, `on_mouse_up`
- touch/gesture:
  `on_touch`, `on_gesture`
- fd integration:
  `on_fd_ready`

## Pointer, mouse, touch semantics

- Pointer events represent all pointing devices (`mouse`, `pen`, `touch`).
- Mouse events are mouse-only.
- Touch input also produces pointer-style events so you can build one unified interaction path if desired.

## Monitor layout APIs

From event context, you can:
- query monitors: `monitors()`, `monitor(id)`
- reposition monitors: `set_monitor_position(id, x, y)`
- apply default horizontal layout: `apply_horizontal_layout()`
- read cursor position in global layout space: `cursor_position()`

Layout validation enforces:
- no overlapping monitor areas
- monitors touching by edges
- no disconnected monitor islands

## Session control APIs

From event context, you can:
- send readiness: `session_ready()`
- query current session: `session()`
- create a session: `create_session(...)`
- switch session: `switch_session(...)`

## Examples

See:
- `app-framework/examples/minimal-gl`

The example shows:
- eager rendering
- pointer/mouse callbacks
- drawing a custom cursor indicator

## Generate API docs

From repo root:

```bash
cargo doc -p tab-app-framework --no-deps
```

Then open:

`target/doc/tab_app_framework/index.html`
