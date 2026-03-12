#!/usr/bin/env bash
# oobo installer
#
# Human usage:
#   curl -fsSL https://oobo.ai/install.sh | bash
#
# Agent usage:
#   curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent
#
# Environment variables:
#   OOBO_INSTALL_DIR    — override install directory (default: ~/.oobo/bin)
#   OOBO_VERSION        — install a specific version (default: latest)
#   OOBO_NO_MODIFY_PATH — set to 1 to skip PATH modification

set -euo pipefail

REPO="ooboai/oobo"
INSTALL_DIR="${OOBO_INSTALL_DIR:-$HOME/.oobo/bin}"
BINARY_NAME="oobo"
AGENT_MODE=0

for arg in "$@"; do
    case "$arg" in
        --agent) AGENT_MODE=1 ;;
    esac
done

# ── Output helpers ────────────────────────────────────────────────────────────

if [[ "$AGENT_MODE" == "1" ]]; then
    RED='' GREEN='' YELLOW='' BLUE='' BOLD='' RESET=''
    info()  { :; }
    ok()    { :; }
    warn()  { echo "warn: $*" >&2; }
    error() { echo "{\"error\":\"$*\"}" >&2; exit 1; }
else
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BLUE='\033[0;34m'
    BOLD='\033[1m'
    RESET='\033[0m'
    info()  { echo -e "${BLUE}${BOLD}info${RESET}  $*"; }
    ok()    { echo -e "${GREEN}${BOLD}ok${RESET}    $*"; }
    warn()  { echo -e "${YELLOW}${BOLD}warn${RESET}  $*"; }
    error() { echo -e "${RED}${BOLD}error${RESET} $*" >&2; exit 1; }
fi

# ── Platform Detection ───────────────────────────────────────────────────────

detect_platform() {
    local os arch

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin)  os="apple-darwin" ;;
        Linux)
            if ldd --version 2>&1 | grep -qi musl || [ -f /etc/alpine-release ]; then
                os="unknown-linux-musl"
            else
                os="unknown-linux-gnu"
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            error "Windows is not yet supported. Build from source: https://github.com/ooboai/oobo#build-from-source"
            ;;
        *)
            error "Unsupported operating system: $os"
            ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)  arch="aarch64" ;;
        *)
            error "Unsupported architecture: $arch"
            ;;
    esac

    echo "${arch}-${os}"
}

# ── Version Detection ────────────────────────────────────────────────────────

get_latest_version() {
    if command -v curl &>/dev/null; then
        curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' \
            | head -1 \
            | sed -E 's/.*"tag_name":\s*"([^"]+)".*/\1/'
    elif command -v wget &>/dev/null; then
        wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' \
            | head -1 \
            | sed -E 's/.*"tag_name":\s*"([^"]+)".*/\1/'
    else
        error "Neither curl nor wget found. Install one and retry."
    fi
}

# ── Download ─────────────────────────────────────────────────────────────────

download() {
    local url="$1" dest="$2"

    if command -v curl &>/dev/null; then
        curl -fsSL "$url" -o "$dest"
    elif command -v wget &>/dev/null; then
        wget -qO "$dest" "$url"
    else
        error "Neither curl nor wget found."
    fi
}

# ── PATH Management ─────────────────────────────────────────────────────────

add_to_path() {
    local dir="$1"

    if [[ "${OOBO_NO_MODIFY_PATH:-0}" == "1" ]]; then
        return
    fi

    # Already in PATH?
    if echo "$PATH" | tr ':' '\n' | grep -qx "$dir"; then
        return
    fi

    local shell_name rc_file export_line
    shell_name="$(basename "${SHELL:-/bin/sh}")"
    export_line="export PATH=\"${dir}:\$PATH\" # oobo"

    case "$shell_name" in
        zsh)
            rc_file="$HOME/.zshrc"
            ;;
        bash)
            if [[ -f "$HOME/.bash_profile" ]]; then
                rc_file="$HOME/.bash_profile"
            else
                rc_file="$HOME/.bashrc"
            fi
            ;;
        fish)
            rc_file="$HOME/.config/fish/config.fish"
            export_line="set -gx PATH ${dir} \$PATH # oobo"
            ;;
        *)
            rc_file="$HOME/.profile"
            ;;
    esac

    if [[ -f "$rc_file" ]] && grep -q "# oobo" "$rc_file" 2>/dev/null; then
        return
    fi

    echo "" >> "$rc_file"
    echo "$export_line" >> "$rc_file"
    info "Added ${dir} to PATH in ${rc_file}"
}

# ── Main ─────────────────────────────────────────────────────────────────────

main() {
    local platform version archive_name url

    if [[ "$AGENT_MODE" != "1" ]]; then
        echo ""
        echo -e "${BOLD}  oobo installer${RESET}"
        echo "  ──────────────────"
        echo ""
    fi

    platform="$(detect_platform)"
    info "Detected platform: ${platform}"

    version="${OOBO_VERSION:-}"
    if [[ -z "$version" ]]; then
        info "Fetching latest version..."
        version="$(get_latest_version)"
        if [[ -z "$version" ]]; then
            error "Could not determine latest version. Set OOBO_VERSION manually."
        fi
    fi
    info "Version: ${version}"

    archive_name="oobo-${version}-${platform}.tar.gz"
    url="https://github.com/${REPO}/releases/download/${version}/${archive_name}"

    info "Downloading ${url}..."
    _oobo_tmpdir="$(mktemp -d)"
    trap 'rm -rf "$_oobo_tmpdir"' EXIT

    download "$url" "${_oobo_tmpdir}/${archive_name}"

    if [[ ! -s "${_oobo_tmpdir}/${archive_name}" ]]; then
        error "Download failed or produced an empty file. Check the URL: ${url}"
    fi

    info "Extracting..."
    tar -xzf "${_oobo_tmpdir}/${archive_name}" -C "$_oobo_tmpdir"

    mkdir -p "$INSTALL_DIR"
    mv "${_oobo_tmpdir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    ok "Installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}"

    add_to_path "$INSTALL_DIR"

    # Make oobo available in this shell session immediately
    export PATH="${INSTALL_DIR}:$PATH"

    if [[ "$AGENT_MODE" == "1" ]]; then
        # Agent mode: output structured JSON result
        cat <<EOF
{"status":"ok","version":"${version}","binary":"${INSTALL_DIR}/${BINARY_NAME}","platform":"${platform}"}
EOF
    else
        echo ""
        echo -e "${GREEN}${BOLD}  Installation complete!${RESET}"
        echo ""
        echo "  To get started:"
        echo "    1. Restart your shell or run:  source ~/.zshrc  (or your shell's rc file)"
        echo "    2. Run:  oobo setup"
        echo ""
        echo "  Quick reference:"
        echo "    oobo sessions list    — view AI chat sessions"
        echo "    oobo dash             — check configuration"
        echo "    oobo alias install    — make 'git' use oobo transparently"
        echo ""
    fi
}

main "$@"
