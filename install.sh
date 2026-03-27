#!/usr/bin/env bash
# ============================================================================
# forge 安装/更新脚本
# ============================================================================
#
# 用法：
#   ./install.sh                安装（自动选择最佳方式）
#   ./install.sh --build        强制从源码构建安装（需要 Rust）
#   ./install.sh --uninstall    卸载
#   ./install.sh --check        检查安装状态
#
# 安装策略（按优先级）：
#   1. GitHub Releases 下载预编译二进制（需要网络）
#   2. dist/ 目录下的预编译二进制（团队共享场景）
#   3. target/release/ 下已有的构建产物（本地已 build 过）
#   4. 从源码构建（需要 Rust）
#
# 安装位置：
#   二进制：~/.forge/bin/fr
#   可通过 FORGE_HOME 环境变量自定义（默认 ~/.forge）

set -euo pipefail

# ─── 配置 ───────────────────────────────────────────────────────────────────

FORGE_HOME="${FORGE_HOME:-$HOME/.forge}"
INSTALL_DIR="$FORGE_HOME/bin"
BACKUP_DIR="$FORGE_HOME/backup"
BINARY_NAME="fr"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MIN_DISK_MB=100

GITHUB_REPO="liuxin231/forge"
GITHUB_API="https://api.github.com/repos/${GITHUB_REPO}/releases/latest"
GITHUB_RELEASES="https://github.com/${GITHUB_REPO}/releases"

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
        *)
            err "Unsupported OS: $os"
            exit 1
            ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        arm64|aarch64)  arch="aarch64" ;;
        *)
            err "Unsupported arch: $arch"
            exit 1
            ;;
    esac

    PLATFORM="${arch}-${os}"
}

# ─── 基础环境检查 ──────────────────────────────────────────────────────────

check_basics() {
    if [[ -z "${HOME:-}" ]] || [[ ! -d "$HOME" ]]; then
        err "HOME is not set or does not exist"
        exit 1
    fi

    if command -v df &>/dev/null; then
        local avail_mb
        avail_mb=$(df -m "$HOME" 2>/dev/null | awk 'NR==2 {print $4}' || echo "9999")
        if [[ "$avail_mb" -lt "$MIN_DISK_MB" ]] 2>/dev/null; then
            err "Insufficient disk space: ${avail_mb}MB available, ${MIN_DISK_MB}MB required"
            exit 1
        fi
    fi
}

# ─── 检测可用下载工具 ──────────────────────────────────────────────────────

detect_downloader() {
    if command -v curl &>/dev/null; then
        DOWNLOADER="curl"
    elif command -v wget &>/dev/null; then
        DOWNLOADER="wget"
    else
        DOWNLOADER=""
    fi
}

http_get() {
    local url="$1"
    case "$DOWNLOADER" in
        curl)  curl -fsSL "$url" ;;
        wget)  wget -qO- "$url" ;;
        *)     err "Neither curl nor wget found"; exit 1 ;;
    esac
}

http_download() {
    local url="$1" dest="$2"
    case "$DOWNLOADER" in
        curl)  curl -fsSL -o "$dest" "$url" ;;
        wget)  wget -qO "$dest" "$url" ;;
        *)     err "Neither curl nor wget found"; exit 1 ;;
    esac
}

# ─── 从 GitHub Releases 下载 ──────────────────────────────────────────────

download_from_github() {
    if [[ -z "$DOWNLOADER" ]]; then
        return 1
    fi

    info "Checking GitHub for latest release..."

    local latest_json
    if ! latest_json=$(http_get "$GITHUB_API" 2>/dev/null); then
        warn "Failed to reach GitHub API"
        return 1
    fi

    # 解析 tag_name 和对应平台的 asset URL
    local tag
    tag=$(echo "$latest_json" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    if [[ -z "$tag" ]]; then
        warn "Could not parse latest release tag"
        return 1
    fi

    info "Latest release: $tag"

    # 查找对应平台的 asset
    local asset_name="fr-${PLATFORM}.tar.gz"
    local asset_url
    asset_url=$(echo "$latest_json" | grep "browser_download_url" | grep "$asset_name" | head -1 \
        | sed 's/.*"browser_download_url": *"\([^"]*\)".*/\1/')

    if [[ -z "$asset_url" ]]; then
        warn "No release asset found for platform: $PLATFORM"
        warn "Available assets at: $GITHUB_RELEASES/tag/$tag"
        return 1
    fi

    # 下载到临时目录（结果通过全局变量 GITHUB_BIN_PATH / DOWNLOAD_TMP_DIR 返回）
    DOWNLOAD_TMP_DIR=$(mktemp -d)
    local tmp_archive="$DOWNLOAD_TMP_DIR/$asset_name"
    info "Downloading $asset_name..."

    if ! http_download "$asset_url" "$tmp_archive"; then
        warn "Download failed"
        rm -rf "$DOWNLOAD_TMP_DIR"
        DOWNLOAD_TMP_DIR=""
        return 1
    fi

    # 校验 checksum（如有 checksums.txt）
    verify_checksum_if_available "$latest_json" "$DOWNLOAD_TMP_DIR" "$asset_name" "$tmp_archive"

    # 解压
    tar -xzf "$tmp_archive" -C "$DOWNLOAD_TMP_DIR"

    GITHUB_BIN_PATH="$DOWNLOAD_TMP_DIR/$BINARY_NAME"
    if [[ ! -f "$GITHUB_BIN_PATH" ]]; then
        warn "Binary not found in archive"
        rm -rf "$DOWNLOAD_TMP_DIR"
        DOWNLOAD_TMP_DIR=""
        return 1
    fi

    # 记录版本标签到环境（供后续显示）
    INSTALLED_VERSION="$tag"
}

# ─── Checksum 校验 ────────────────────────────────────────────────────────

verify_checksum_if_available() {
    local release_json="$1" tmp_dir="$2" asset_name="$3" archive_path="$4"

    # 需要 sha256sum 或 shasum
    local sha_cmd=""
    if command -v sha256sum &>/dev/null; then
        sha_cmd="sha256sum"
    elif command -v shasum &>/dev/null; then
        sha_cmd="shasum -a 256"
    else
        warn "sha256sum/shasum not found, skipping checksum verification"
        return 0
    fi

    # 查找 checksums.txt 下载链接
    local checksums_url
    checksums_url=$(echo "$release_json" | grep "browser_download_url" | grep "checksums.txt" | head -1 \
        | sed 's/.*"browser_download_url": *"\([^"]*\)".*/\1/')

    if [[ -z "$checksums_url" ]]; then
        warn "checksums.txt not found in release, skipping integrity check"
        return 0
    fi

    local checksums_file="$tmp_dir/checksums.txt"
    if ! http_download "$checksums_url" "$checksums_file" 2>/dev/null; then
        warn "Failed to download checksums.txt, skipping integrity check"
        return 0
    fi

    # 从 checksums.txt 提取期望的 hash
    local expected_hash
    expected_hash=$(grep "$asset_name" "$checksums_file" | awk '{print $1}')

    if [[ -z "$expected_hash" ]]; then
        warn "No checksum entry for $asset_name, skipping integrity check"
        return 0
    fi

    # 计算实际 hash
    local actual_hash
    actual_hash=$($sha_cmd "$archive_path" | awk '{print $1}')

    if [[ "$actual_hash" != "$expected_hash" ]]; then
        err "Checksum mismatch!"
        err "  Expected: $expected_hash"
        err "  Actual:   $actual_hash"
        err "The downloaded file may be corrupted or tampered with."
        rm -rf "$tmp_dir"
        exit 1
    fi

    ok "Checksum verified (SHA256)"
}

# ─── 查找本地预编译二进制 ──────────────────────────────────────────────────

find_local_prebuilt() {
    # dist/<name>-<platform>
    local dist_bin="$SCRIPT_DIR/dist/${BINARY_NAME}-${PLATFORM}"
    if [[ -f "$dist_bin" ]]; then
        echo "$dist_bin"; return 0
    fi

    # dist/<name>（通用）
    dist_bin="$SCRIPT_DIR/dist/${BINARY_NAME}"
    if [[ -f "$dist_bin" ]]; then
        echo "$dist_bin"; return 0
    fi

    # 本地已构建
    local target_bin="$SCRIPT_DIR/target/release/${BINARY_NAME}"
    if [[ -f "$target_bin" ]]; then
        echo "$target_bin"; return 0
    fi

    return 1
}

# ─── 源码构建 ──────────────────────────────────────────────────────────────

build_from_source() {
    if [[ ! -f "$SCRIPT_DIR/Cargo.toml" ]]; then
        err "Cargo.toml not found — cannot build from source"
        err "Run from the repo root, or use a prebuilt binary"
        exit 1
    fi

    if ! command -v cargo &>/dev/null; then
        if [[ -f "$HOME/.cargo/env" ]]; then
            # shellcheck disable=SC1091
            source "$HOME/.cargo/env"
        fi
    fi

    if ! command -v cargo &>/dev/null; then
        echo ""
        warn "Rust toolchain not found (needed for source build)"
        echo ""
        echo "  1) Install Rust automatically"
        echo "  2) Exit"
        echo ""
        read -rp "Choice [1/2]: " choice
        case "${choice}" in
            1)
                info "Installing Rust via rustup..."
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
                info "Tip: download a prebuilt binary from $GITHUB_RELEASES"
                exit 0
                ;;
        esac
    fi

    info "Building from source (release mode)..."
    cd "$SCRIPT_DIR"

    if ! cargo build --release 2>&1; then
        err "Build failed. Try: rustup update && cargo clean && ./install.sh --build"
        exit 1
    fi

    local bin="$SCRIPT_DIR/target/release/$BINARY_NAME"
    if [[ ! -f "$bin" ]]; then
        err "Build succeeded but binary not found: $bin"
        exit 1
    fi

    echo "$bin"
}

# ─── 命令名冲突检查 ────────────────────────────────────────────────────────

check_conflict() {
    local existing
    existing=$(command -v "$BINARY_NAME" 2>/dev/null || true)
    if [[ -n "$existing" ]] && [[ "$existing" != "$INSTALL_DIR/$BINARY_NAME" ]]; then
        warn "Existing '$BINARY_NAME' found at: $existing"
        warn "After install, $INSTALL_DIR/$BINARY_NAME will take precedence if it appears earlier in PATH"
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
        *)      warn "Add manually: export PATH=\"\$HOME/.forge/bin:\$PATH\""; return 0 ;;
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

    if [[ -f "$INSTALL_DIR/$BINARY_NAME" ]]; then
        mkdir -p "$BACKUP_DIR"
        local bak="${BINARY_NAME}.$(date +%Y%m%d%H%M%S).bak"
        cp "$INSTALL_DIR/$BINARY_NAME" "$BACKUP_DIR/$bak"
        info "Backed up previous version: $BACKUP_DIR/$bak"
        # 保留最近 5 个备份
        ls -1t "$BACKUP_DIR/${BINARY_NAME}".*.bak 2>/dev/null | tail -n +6 | xargs rm -f 2>/dev/null || true
    fi

    local tmp="$INSTALL_DIR/${BINARY_NAME}.tmp.$$"
    if ! cp "$src" "$tmp"; then
        err "Failed to copy binary to $INSTALL_DIR (permission denied?)"
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
    detect_downloader
    info "Platform: $PLATFORM"

    INSTALLED_VERSION=""
    DOWNLOAD_TMP_DIR=""
    GITHUB_BIN_PATH=""
    local bin_path=""

    if $force_build; then
        bin_path=$(build_from_source)
    else
        # 1. GitHub Releases（不用 command substitution，结果通过全局变量返回）
        if download_from_github 2>/dev/null; then
            bin_path="$GITHUB_BIN_PATH"
            ok "Downloaded from GitHub Releases"
        # 2. 本地预编译
        elif bin_path=$(find_local_prebuilt); then
            ok "Using local prebuilt: $bin_path"
        # 3. 源码构建
        else
            info "No prebuilt binary found, building from source..."
            bin_path=$(build_from_source)
        fi
    fi

    atomic_install "$bin_path"

    # 清理下载临时目录
    [[ -n "$DOWNLOAD_TMP_DIR" && -d "$DOWNLOAD_TMP_DIR" ]] && rm -rf "$DOWNLOAD_TMP_DIR"

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
        warn "Not installed at $INSTALL_DIR/$BINARY_NAME"
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
    detect_downloader
    info "Platform: $PLATFORM"

    if [[ -f "$INSTALL_DIR/$BINARY_NAME" ]]; then
        ok "Binary:  $INSTALL_DIR/$BINARY_NAME"
        if "$INSTALL_DIR/$BINARY_NAME" --version &>/dev/null; then
            ok "Version: $("$INSTALL_DIR/$BINARY_NAME" --version)"
        fi
    else
        err "Binary: not installed"
    fi

    local which_path
    which_path=$(command -v "$BINARY_NAME" 2>/dev/null || true)
    if [[ "$which_path" == "$INSTALL_DIR/$BINARY_NAME" ]]; then
        ok "PATH:    correct"
    elif [[ -n "$which_path" ]]; then
        warn "PATH:    $which_path (not from forge install dir)"
    else
        err "PATH:    '$BINARY_NAME' not in PATH — add $INSTALL_DIR to PATH"
    fi

    if [[ -n "$DOWNLOADER" ]]; then
        info "Downloader: $DOWNLOADER"
        local latest_tag
        latest_tag=$(http_get "$GITHUB_API" 2>/dev/null | grep '"tag_name"' | head -1 \
            | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' || true)
        [[ -n "$latest_tag" ]] && info "Latest release: $latest_tag"
    else
        warn "Downloader: none (curl/wget not found, cannot check GitHub)"
    fi

    if command -v cargo &>/dev/null; then
        info "Cargo: $(cargo --version) (available for --build)"
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
        echo "  (no args)         Install fr (GitHub Release → local prebuilt → source build)"
        echo "  --build,  -b      Force build from source (requires Rust)"
        echo "  --check,  -c      Check install status and latest release"
        echo "  --uninstall, -u   Remove fr"
        echo "  --help,   -h      Show this help"
        echo ""
        echo "Install strategy (in order):"
        echo "  1. GitHub Releases fr-<platform>.tar.gz  (requires curl/wget)"
        echo "  2. dist/${BINARY_NAME}-<platform>                 (team-shared prebuilt)"
        echo "  3. dist/${BINARY_NAME}                             (generic prebuilt)"
        echo "  4. target/release/${BINARY_NAME}                  (already built locally)"
        echo "  5. cargo build --release                  (build from source)"
        echo ""
        echo "Environment:"
        echo "  FORGE_HOME    Install root (default: ~/.forge)"
        echo ""
        echo "Rollback:"
        echo "  cp ~/.forge/backup/fr.<timestamp>.bak ~/.forge/bin/fr"
        ;;
    "")             do_install ;;
    *)              err "Unknown option: $1 (try --help)"; exit 1 ;;
esac
