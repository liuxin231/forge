#!/usr/bin/env bash
# ============================================================================
# forge 离线安装/更新脚本
# ============================================================================
#
# 用法：
#   ./install.sh                安装（自动选择最佳方式）
#   ./install.sh --build        强制从源码构建安装（需要 Rust）
#   ./install.sh --uninstall    卸载
#   ./install.sh --check        检查安装状态
#
# 安装策略（按优先级）：
#   1. dist/ 目录下的预编译二进制（团队共享场景）
#   2. target/release/ 下已有的构建产物（开发者本地已 build 过）
#   3. 从源码构建（需要 Rust，自动提示安装）
#
# 安装位置：
#   二进制：~/.forge/bin/fr
#   可通过 FORGE_HOME 环境变量自定义（默认 ~/.forge）
#
# 更新：
#   git pull && ./install.sh

set -euo pipefail

# ─── 配置 ───────────────────────────────────────────────────────────────────

FORGE_HOME="${FORGE_HOME:-$HOME/.forge}"
INSTALL_DIR="$FORGE_HOME/bin"
BACKUP_DIR="$FORGE_HOME/backup"
BINARY_NAME="fr"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MIN_DISK_MB=500

# ─── 颜色（自动检测终端能力） ──────────────────────────────────────────────

if [[ -t 1 ]] && [[ "${TERM:-}" != "dumb" ]]; then
    RED='\033[0;31m' GREEN='\033[0;32m' YELLOW='\033[0;33m'
    CYAN='\033[0;36m' BOLD='\033[1m' NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' BOLD='' NC=''
fi

info()  { echo -e "${CYAN}[info]${NC}  $*"; }
ok()    { echo -e "${GREEN}[ok]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[warn]${NC}  $*"; }
err()   { echo -e "${RED}[error]${NC} $*" >&2; }

# ─── 平台检测 ──────────────────────────────────────────────────────────────

detect_platform() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin) os="apple-darwin" ;;
        Linux)  os="unknown-linux-gnu" ;;
        *)      os="unknown" ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        arm64|aarch64)  arch="aarch64" ;;
        *)              arch="unknown" ;;
    esac

    PLATFORM="${arch}-${os}"
}

# ─── 基础环境检查 ──────────────────────────────────────────────────────────

check_basics() {
    if [[ -z "${HOME:-}" ]] || [[ ! -d "$HOME" ]]; then
        err "HOME is not set or does not exist"
        exit 1
    fi

    # 磁盘空间
    if command -v df &>/dev/null; then
        local avail_mb
        avail_mb=$(df -m "$HOME" 2>/dev/null | awk 'NR==2 {print $4}' || echo "9999")
        if [[ "$avail_mb" -lt "$MIN_DISK_MB" ]] 2>/dev/null; then
            err "Insufficient disk space: ${avail_mb}MB available, ${MIN_DISK_MB}MB required"
            exit 1
        fi
    fi
}

# ─── 查找预编译二进制 ──────────────────────────────────────────────────────

find_prebuilt() {
    # 策略 1：dist/ 目录（按平台查找）
    local dist_bin="$SCRIPT_DIR/dist/${BINARY_NAME}-${PLATFORM}"
    if [[ -f "$dist_bin" ]]; then
        echo "$dist_bin"
        return 0
    fi

    # dist/ 通用名（无平台后缀）
    dist_bin="$SCRIPT_DIR/dist/${BINARY_NAME}"
    if [[ -f "$dist_bin" ]]; then
        echo "$dist_bin"
        return 0
    fi

    # 策略 2：target/release/（本地已构建过）
    local target_bin="$SCRIPT_DIR/target/release/${BINARY_NAME}"
    if [[ -f "$target_bin" ]]; then
        echo "$target_bin"
        return 0
    fi

    return 1
}

# ─── 源码构建 ──────────────────────────────────────────────────────────────

build_from_source() {
    # 检查 Cargo.toml
    if [[ ! -f "$SCRIPT_DIR/Cargo.toml" ]]; then
        err "Cargo.toml not found — cannot build from source"
        err "Either place a prebuilt binary in dist/ or clone the full repo"
        exit 1
    fi

    # 检查 Rust
    if ! command -v cargo &>/dev/null; then
        # 尝试加载 cargo env
        if [[ -f "$HOME/.cargo/env" ]]; then
            # shellcheck disable=SC1091
            source "$HOME/.cargo/env"
        fi
    fi

    if ! command -v cargo &>/dev/null; then
        echo ""
        warn "Rust toolchain not found (needed for source build)"
        echo ""
        echo "Options:"
        echo "  1) Install Rust now (automatic)"
        echo "  2) Exit — get a prebuilt binary from a teammate and put it in dist/"
        echo ""
        read -rp "Choice [1/2]: " choice

        case "${choice}" in
            1)
                info "Installing Rust..."
                if curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; then
                    # shellcheck disable=SC1091
                    source "$HOME/.cargo/env"
                    ok "Rust installed: $(cargo --version)"
                else
                    err "Rust installation failed"
                    exit 1
                fi
                ;;
            *)
                info "To share a prebuilt binary:"
                info "  # On a machine with Rust:"
                info "  cargo build --release"
                info "  mkdir -p dist && cp target/release/$BINARY_NAME dist/"
                info ""
                info "  # Then copy dist/$BINARY_NAME to this machine"
                exit 0
                ;;
        esac
    fi

    info "Building from source (release mode)..."
    cd "$SCRIPT_DIR"

    if ! cargo build --release 2>&1; then
        err "Build failed"
        err "  Try: rustup update && cargo clean && ./install.sh --build"
        exit 1
    fi

    local bin="$SCRIPT_DIR/target/release/$BINARY_NAME"
    if [[ ! -f "$bin" ]]; then
        err "Build succeeded but binary not found: $bin"
        err "Check [[bin]] name in Cargo.toml"
        exit 1
    fi

    echo "$bin"
}

# ─── 命令名冲突检查 ────────────────────────────────────────────────────────

check_conflict() {
    local existing
    existing=$(command -v "$BINARY_NAME" 2>/dev/null || true)
    if [[ -n "$existing" ]] && [[ "$existing" != "$INSTALL_DIR/$BINARY_NAME" ]]; then
        warn "Existing '$BINARY_NAME' at: $existing (will be superseded by $INSTALL_DIR/$BINARY_NAME)"
    fi
}

# ─── PATH 配置 ──────────────────────────────────────────────────────────────

ensure_path() {
    if [[ ":$PATH:" == *":$INSTALL_DIR:"* ]]; then
        return 0
    fi

    warn "$INSTALL_DIR is not in PATH"

    local shell_rc=""
    case "${SHELL:-}" in
        */zsh)  shell_rc="$HOME/.zshrc" ;;
        */bash) shell_rc="${HOME}/.bash_profile"; [[ -f "$shell_rc" ]] || shell_rc="$HOME/.bashrc" ;;
        */fish) shell_rc="$HOME/.config/fish/config.fish" ;;
        *)      warn "Add to PATH manually: export PATH=\"\$HOME/.forge/bin:\$PATH\""; return 0 ;;
    esac

    [[ -f "$shell_rc" ]] || touch "$shell_rc"

    if grep -qF '.forge/bin' "$shell_rc" 2>/dev/null; then
        return 0
    fi

    local path_line='export PATH="$HOME/.forge/bin:$PATH"'
    [[ "${SHELL:-}" == */fish ]] && path_line='set -gx PATH $HOME/.forge/bin $PATH'

    printf '\n# forge\n%s\n' "$path_line" >> "$shell_rc"
    ok "Added PATH to $shell_rc"
    warn "Run: source $shell_rc  (or restart terminal)"
}

# ─── 原子安装（备份 + tmp + mv）──────────────────────────────────────────

atomic_install() {
    local src="$1"

    mkdir -p "$INSTALL_DIR"

    # 备份旧版本
    if [[ -f "$INSTALL_DIR/$BINARY_NAME" ]]; then
        mkdir -p "$BACKUP_DIR"
        local bak="${BINARY_NAME}.$(date +%Y%m%d%H%M%S).bak"
        cp "$INSTALL_DIR/$BINARY_NAME" "$BACKUP_DIR/$bak"
        info "Backed up: $BACKUP_DIR/$bak"

        # 保留最近 5 个
        ls -1t "$BACKUP_DIR/${BINARY_NAME}".*.bak 2>/dev/null | tail -n +6 | xargs rm -f 2>/dev/null || true
    fi

    # 原子写入
    local tmp="$INSTALL_DIR/${BINARY_NAME}.tmp.$$"
    if ! cp "$src" "$tmp"; then
        err "Failed to copy to $INSTALL_DIR (permission denied?)"
        rm -f "$tmp"
        exit 1
    fi
    chmod +x "$tmp"

    if ! mv "$tmp" "$INSTALL_DIR/$BINARY_NAME"; then
        err "Failed to install (mv failed)"
        rm -f "$tmp"
        exit 1
    fi

    ok "Installed: $INSTALL_DIR/$BINARY_NAME"
}

# ─── 安装主流程 ────────────────────────────────────────────────────────────

do_install() {
    local force_build=false
    [[ "${1:-}" == "--build" ]] && force_build=true

    echo -e "${BOLD}forge installer${NC}"
    echo ""

    check_basics
    check_conflict
    detect_platform
    info "Platform: $PLATFORM"

    local bin_path=""

    if $force_build; then
        # 强制源码构建
        bin_path=$(build_from_source)
    else
        # 优先查找预编译
        if bin_path=$(find_prebuilt); then
            ok "Found prebuilt: $bin_path"
        else
            info "No prebuilt binary found, building from source..."
            bin_path=$(build_from_source)
        fi
    fi

    # 安装
    atomic_install "$bin_path"

    # 版本
    if "$INSTALL_DIR/$BINARY_NAME" --version &>/dev/null; then
        ok "Version: $("$INSTALL_DIR/$BINARY_NAME" --version)"
    fi

    ensure_path

    echo ""
    ok "Done! Run 'fr --help' to get started."
}

# ─── 卸载 ───────────────────────────────────────────────────────────────────

do_uninstall() {
    echo -e "${BOLD}forge uninstaller${NC}"
    echo ""

    if [[ -f "$INSTALL_DIR/$BINARY_NAME" ]]; then
        rm "$INSTALL_DIR/$BINARY_NAME"
        ok "Removed: $INSTALL_DIR/$BINARY_NAME"
    else
        warn "Not installed"
    fi

    [[ -d "$BACKUP_DIR" ]] && rm -rf "$BACKUP_DIR" && ok "Removed backups"

    echo ""
    info "Optional cleanup:"
    [[ -d "$FORGE_HOME" ]] && info "  rm -rf $FORGE_HOME"

    for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile"; do
        if [[ -f "$rc" ]] && grep -qF '.forge/bin' "$rc" 2>/dev/null; then
            info "  Remove '.forge/bin' line from $rc"
        fi
    done
}

# ─── 状态检查 ──────────────────────────────────────────────────────────────

do_check() {
    echo -e "${BOLD}forge status${NC}"
    echo ""

    detect_platform
    info "Platform: $PLATFORM"

    # 二进制
    if [[ -f "$INSTALL_DIR/$BINARY_NAME" ]]; then
        ok "Binary: $INSTALL_DIR/$BINARY_NAME"
        if "$INSTALL_DIR/$BINARY_NAME" --version &>/dev/null; then
            ok "Version: $("$INSTALL_DIR/$BINARY_NAME" --version)"
        fi
    else
        err "Binary: not installed"
    fi

    # PATH
    local which_path
    which_path=$(command -v "$BINARY_NAME" 2>/dev/null || true)
    if [[ "$which_path" == "$INSTALL_DIR/$BINARY_NAME" ]]; then
        ok "PATH: correct"
    elif [[ -n "$which_path" ]]; then
        warn "PATH: $which_path (not from forge install)"
    else
        err "PATH: '$BINARY_NAME' not in PATH"
    fi

    # 预编译
    if find_prebuilt &>/dev/null; then
        ok "Prebuilt: available"
    else
        info "Prebuilt: not found (will build from source on install)"
    fi

    # Rust（仅作为信息展示，不是必须）
    if command -v cargo &>/dev/null; then
        ok "Cargo: $(cargo --version) (optional, for source builds)"
    else
        info "Cargo: not installed (only needed for --build)"
    fi
}

# ─── 入口 ───────────────────────────────────────────────────────────────────

case "${1:-}" in
    --build|-b)     do_install --build ;;
    --uninstall|-u) do_uninstall ;;
    --check|-c)     do_check ;;
    --help|-h)
        echo -e "${BOLD}forge installer${NC}"
        echo ""
        echo "Usage: $0 [OPTION]"
        echo ""
        echo "  (no args)       Install fr (prebuilt or source)"
        echo "  --build,  -b    Force build from source (requires Rust)"
        echo "  --check,  -c    Check installation status"
        echo "  --uninstall, -u Remove fr"
        echo "  --help,  -h     Show this help"
        echo ""
        echo "Install strategy (in order):"
        echo "  1. dist/$BINARY_NAME-<platform>   Prebuilt for your platform"
        echo "  2. dist/$BINARY_NAME              Prebuilt (generic)"
        echo "  3. target/release/$BINARY_NAME    Already built locally"
        echo "  4. cargo build --release          Build from source"
        echo ""
        echo "Share prebuilt (no Rust needed on target machine):"
        echo "  cargo build --release"
        echo "  mkdir -p dist && cp target/release/$BINARY_NAME dist/"
        echo "  # copy dist/ to target machine, then ./install.sh"
        echo ""
        echo "Environment:"
        echo "  FORGE_HOME    Install root (default: ~/.forge)"
        echo ""
        echo "Rollback:"
        echo "  cp ~/.forge/backup/fr.<timestamp>.bak ~/.forge/bin/fr"
        ;;
    "")             do_install ;;
    *)              err "Unknown: $1 (try --help)"; exit 1 ;;
esac
