set dotenv-load

PROFILING_BASE_DIR := "profiling"
TIMESTAMP := `date +%Y-%m-%d_%H-%M-%S`

default:
    @just --choose

# Compila o Shift mantendo os símbolos de debug
build-with-debug-symbols:
    @echo "🛠️ Compilando Shift com símbolos de debug..."
    cargo build --profile release-with-debug

run: build-with-debug-symbols
    #!/usr/bin/env bash
    set -euo pipefail

    if [ -z "$HYPRLAND_BIN" ]; then 
        echo "❌ Erro: \$HYPRLAND_BIN não definida no .env"
        exit 1
    fi
    export ADMIN_LAUNCH_CMD="sleep 2s && TRACY_PORT=1234 $HYPRLAND_BIN"
    cargo run --bin shift --profile release-with-debug


# Workflow de Profiling Unificado
profile: build-with-debug-symbols
    #!/usr/bin/env bash
    set -euo pipefail

    if [ -z "$HYPRLAND_BIN" ]; then 
        echo "❌ Erro: \$HYPRLAND_BIN não definida no .env"
        exit 1
    fi

    sudo sysctl -w kernel.perf_event_paranoid=-1
    
    RUN_DIR="{{PROFILING_BASE_DIR}}/run_{{TIMESTAMP}}"
    mkdir -p "$RUN_DIR"
    
    echo "🚀 Iniciando Unified Profiling: $RUN_DIR"
    
    # O cargo-flamegraph herda os filhos, capturando o Hyprland automaticamente
    export ADMIN_LAUNCH_CMD="sleep 0.5s && $HYPRLAND_BIN"
    
    # Usamos o binário do profile release-with-debug (normalmente em target/release-with-debug/shift)
    # O cargo-flamegraph por padrão procura no target/release se usares --bin,
    # por isso passamos o caminho direto se necessário.
    cargo flamegraph --bin shift --output "$RUN_DIR/unified_flame.svg" --profile release-with-debug
    
    echo "✅ Sessão finalizada em $RUN_DIR/unified_flame.svg"

view:
    #!/usr/bin/env bash
    set -euo pipefail

    RUN=$(ls -dt {{PROFILING_BASE_DIR}}/run_* 2>/dev/null | fzf \
        --header "1. SELECIONA A SESSÃO" \
        --preview 'ls -lh {}' \
        --height 40% --reverse) || exit 0
    
    ls "$RUN"/*.svg 2>/dev/null | fzf -m --header "2. ABRIR FLAMEGRAPH" --height 40% --reverse | xargs -r google-chrome-stable

clean:
    rm -rf {{PROFILING_BASE_DIR}}

test-harness switch_ms="2500": build-with-debug-symbols
    #!/usr/bin/env bash
    set -euo pipefail

    if [ -z "${HYPRLAND_BIN:-}" ]; then
        echo "❌ Erro: \$HYPRLAND_BIN não definida no .env"
        exit 1
    fi
    if [ ! -x "$HYPRLAND_BIN" ]; then
        echo "❌ Erro: HYPRLAND_BIN não é executável: $HYPRLAND_BIN"
        exit 1
    fi

    ROOT="$(pwd)"
    TEST_ROOT="$ROOT/test"
    ASSET_ADMIN="$TEST_ROOT/assets/session1.mp4"
    ASSET_SECOND="$TEST_ROOT/assets/session2.gif"
    PLAYER="$TEST_ROOT/scripts/play-media.sh"
    ADMIN_CFG="/tmp/shift-admin-test.conf"
    SECOND_CFG="/tmp/shift-second-test.conf"

    if [ ! -f "$ASSET_ADMIN" ]; then
        echo "❌ Asset ausente: $ASSET_ADMIN"
        exit 1
    fi
    if [ ! -f "$ASSET_SECOND" ]; then
        echo "❌ Asset ausente: $ASSET_SECOND"
        exit 1
    fi
    if [ ! -x "$PLAYER" ]; then
        chmod +x "$PLAYER"
    fi

    printf '%s\n' \
      'monitor=,preferred,auto,1' \
      'misc {' \
      '  disable_hyprland_logo = true' \
      '}' \
      "exec-once = $PLAYER \"$ASSET_ADMIN\"" > "$ADMIN_CFG"

    printf '%s\n' \
      'monitor=,preferred,auto,1' \
      'misc {' \
      '  disable_hyprland_logo = true' \
      '}' \
      "exec-once = $PLAYER \"$ASSET_SECOND\"" > "$SECOND_CFG"

    export SHIFT_DEBUG_AUTO_SWITCH_INTERVAL_MS="{{switch_ms}}"
    export SHIFT_DEBUG_SECOND_SESSION_CMD="$HYPRLAND_BIN --config $SECOND_CFG"
    export ADMIN_LAUNCH_CMD="$HYPRLAND_BIN --config $ADMIN_CFG"

    echo "🚀 Starting Shift test harness"
    echo "   - admin media:  $ASSET_ADMIN"
    echo "   - second media: $ASSET_SECOND"
    echo "   - auto switch:  ${SHIFT_DEBUG_AUTO_SWITCH_INTERVAL_MS}ms"
    cargo run --bin shift --profile release-with-debug
