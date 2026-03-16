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
            | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/'
    elif command -v wget &>/dev/null; then
        wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' \
            | head -1 \
            | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/'
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

# ── Shell RC Detection ───────────────────────────────────────────────────────

detect_rc_file() {
    local shell_name
    shell_name="$(basename "${SHELL:-/bin/sh}")"

    case "$shell_name" in
        zsh)  echo "$HOME/.zshrc" ;;
        bash)
            if [[ -f "$HOME/.bash_profile" ]]; then
                echo "$HOME/.bash_profile"
            elif [[ -f "$HOME/.bashrc" ]]; then
                echo "$HOME/.bashrc"
            else
                echo "$HOME/.profile"
            fi
            ;;
        fish) echo "$HOME/.config/fish/config.fish" ;;
        *)    echo "$HOME/.profile" ;;
    esac
}

# ── PATH Management ─────────────────────────────────────────────────────────

# Check if a directory is already on PATH
is_on_path() {
    echo "$PATH" | tr ':' '\n' | grep -qx "$1" 2>/dev/null
}

# Write an export line to the shell rc file for a given directory
ensure_rc_has_dir() {
    local dir="$1"
    local rc_file="$2"

    if [[ "${OOBO_NO_MODIFY_PATH:-0}" == "1" ]]; then
        return
    fi

    if [[ -f "$rc_file" ]] && grep -q "# oobo" "$rc_file" 2>/dev/null; then
        return
    fi

    local shell_name
    shell_name="$(basename "${SHELL:-/bin/sh}")"

    local export_line
    case "$shell_name" in
        fish) export_line="set -gx PATH ${dir} \$PATH # oobo" ;;
        *)    export_line="export PATH=\"${dir}:\$PATH\" # oobo" ;;
    esac

    echo "" >> "$rc_file"
    echo "$export_line" >> "$rc_file"
    info "Added ${dir} to PATH in ${rc_file}"
}

# Try to place a symlink in a well-known PATH directory so the binary
# is available immediately in the parent shell (no source needed).
# Returns 0 and prints the directory on success, 1 on failure.
try_symlink_to_path() {
    local target="$1"

    # Tier 1: /usr/local/bin (works when root / admin)
    for candidate in /usr/local/bin; do
        if is_on_path "$candidate" && [[ -d "$candidate" ]] && [[ -w "$candidate" ]]; then
            if ln -sf "$target" "${candidate}/${BINARY_NAME}" 2>/dev/null; then
                echo "$candidate"
                return 0
            fi
        fi
    done

    # Tier 2: user-local directory (XDG standard, writable without root)
    local local_bin="$HOME/.local/bin"
    mkdir -p "$local_bin" 2>/dev/null || true
    if [[ -d "$local_bin" ]] && [[ -w "$local_bin" ]]; then
        if ln -sf "$target" "${local_bin}/${BINARY_NAME}" 2>/dev/null; then
            echo "$local_bin"
            return 0
        fi
    fi

    return 1
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

    local rc_file
    rc_file="$(detect_rc_file)"

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

    # Make oobo available for the rest of this script
    export PATH="${INSTALL_DIR}:$PATH"

    # Try to place a symlink in a directory already on PATH so the
    # binary is available in the parent shell without source / restart.
    local link_dir=""
    local needs_source=1

    if link_dir="$(try_symlink_to_path "${INSTALL_DIR}/${BINARY_NAME}")"; then
        ok "Linked ${link_dir}/${BINARY_NAME} → ${INSTALL_DIR}/${BINARY_NAME}"

        if is_on_path "$link_dir"; then
            needs_source=0
        else
            # ~/.local/bin was created but isn't on PATH yet — add it
            ensure_rc_has_dir "$link_dir" "$rc_file"
        fi
    else
        # No symlink target available — fall back to rc file
        ensure_rc_has_dir "$INSTALL_DIR" "$rc_file"
    fi

    if [[ "$AGENT_MODE" == "1" ]]; then
        cat <<EOF
{"status":"ok","version":"${version}","binary":"${INSTALL_DIR}/${BINARY_NAME}","platform":"${platform}"}
EOF
        return 0
    fi

    echo ""
    echo -e "${GREEN}${BOLD}  Installation complete!${RESET}"
    echo ""

    if [[ "$needs_source" == "1" ]]; then
        local rc_short="${rc_file/#$HOME/\~}"
        warn "To use oobo in this shell, run:  source ${rc_short}"
        echo "  (New terminals will pick it up automatically.)"
        echo ""
    fi

    echo "  Quick reference:"
    echo "    oobo sessions list    — view AI chat sessions"
    echo "    oobo dash             — check configuration"
    echo "    oobo alias install    — make 'git' use oobo transparently"
    echo ""

    # Clean up tmpdir before exec (exec replaces the process so the
    # EXIT trap would never fire).
    rm -rf "$_oobo_tmpdir" 2>/dev/null || true
    trap - EXIT

    # Run setup — hand off the process entirely so the TUI wizard
    # gets a real TTY (stdin is consumed by the pipe).
    if [[ -r /dev/tty && -w /dev/tty ]]; then
        echo -e "  Running ${BOLD}oobo setup${RESET}..."
        echo ""
        exec "${INSTALL_DIR}/${BINARY_NAME}" setup </dev/tty
    else
        info "No TTY available — run ${BOLD}oobo setup${RESET} to finish configuration."
    fi
}

main "$@"
