#!/usr/bin/env bash
#
# ASCIIVision Installer
#
# Installs all dependencies, builds the project, and installs the binary
# so you can run "asciivision" from anywhere.
#
# Supported: macOS (Homebrew), Ubuntu/Debian, Fedora/RHEL, Arch Linux
# Windows users: install WSL2 (wsl --install) then run this script inside it.
#
# Usage:
#   curl -sSf <repo-raw-url>/install.sh | bash
#   -- or --
#   git clone <repo-url> && cd asciivision && ./install.sh
#

set -euo pipefail

BOLD="\033[1m"
GREEN="\033[1;32m"
YELLOW="\033[1;33m"
RED="\033[1;31m"
CYAN="\033[1;36m"
RESET="\033[0m"

info()    { printf "${CYAN}[INFO]${RESET}  %s\n" "$*"; }
success() { printf "${GREEN}[OK]${RESET}    %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[ERROR]${RESET} %s\n" "$*" >&2; }

INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# OS detection
# ---------------------------------------------------------------------------
detect_os() {
    local uname_s
    uname_s="$(uname -s)"
    case "${uname_s}" in
        Darwin) OS="macos" ;;
        Linux)  OS="linux" ;;
        MINGW*|MSYS*|CYGWIN*)
            err "Native Windows is not supported. Please use WSL2:"
            err "  1. Open PowerShell as admin and run:  wsl --install"
            err "  2. Restart, open Ubuntu from Start Menu"
            err "  3. Clone this repo inside WSL and run ./install.sh"
            exit 1
            ;;
        *)
            err "Unsupported OS: ${uname_s}"
            exit 1
            ;;
    esac
}

detect_linux_distro() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        case "${ID:-}" in
            ubuntu|debian|pop|linuxmint|elementary|zorin|kali)
                DISTRO="debian" ;;
            fedora|rhel|centos|rocky|alma|nobara)
                DISTRO="fedora" ;;
            arch|manjaro|endeavouros|garuda)
                DISTRO="arch" ;;
            opensuse*|sles)
                DISTRO="suse" ;;
            *)
                DISTRO="unknown" ;;
        esac
    else
        DISTRO="unknown"
    fi
}

# ---------------------------------------------------------------------------
# Dependency checks
# ---------------------------------------------------------------------------
has_cmd() { command -v "$1" >/dev/null 2>&1; }

check_rust() {
    if has_cmd rustc && has_cmd cargo; then
        success "Rust $(rustc --version | awk '{print $2}') found"
        return 0
    fi
    return 1
}

install_rust() {
    info "Installing Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # Source cargo env for the rest of this script
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
    success "Rust installed: $(rustc --version)"
}

# ---------------------------------------------------------------------------
# macOS dependency installation
# ---------------------------------------------------------------------------
install_deps_macos() {
    if ! has_cmd brew; then
        info "Homebrew not found. Installing Homebrew..."
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
        # Add brew to PATH for Apple Silicon and Intel
        if [ -f /opt/homebrew/bin/brew ]; then
            eval "$(/opt/homebrew/bin/brew shellenv)"
        elif [ -f /usr/local/bin/brew ]; then
            eval "$(/usr/local/bin/brew shellenv)"
        fi
        success "Homebrew installed"
    else
        success "Homebrew found"
    fi

    local packages=()

    if ! has_cmd ffmpeg; then
        packages+=(ffmpeg)
    else
        success "FFmpeg found"
    fi

    if ! has_cmd pkg-config; then
        packages+=(pkg-config)
    else
        success "pkg-config found"
    fi

    if ! has_cmd yt-dlp; then
        packages+=(yt-dlp)
    else
        success "yt-dlp found"
    fi

    if ! has_cmd ollama; then
        packages+=(ollama)
    else
        success "Ollama found"
    fi

    # Check for llvm/libclang
    local has_libclang=false
    local search_dirs=(
        "/opt/homebrew/opt/llvm/lib"
        "/usr/local/opt/llvm/lib"
        "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib"
    )
    if has_cmd brew; then
        for formula in llvm llvm@21 llvm@20 llvm@19 llvm@18; do
            local prefix
            prefix="$(brew --prefix "${formula}" 2>/dev/null || true)"
            if [[ -n "${prefix}" && -f "${prefix}/lib/libclang.dylib" ]]; then
                has_libclang=true
                break
            fi
        done
    fi
    if ! $has_libclang; then
        for dir in "${search_dirs[@]}"; do
            if [[ -f "${dir}/libclang.dylib" ]]; then
                has_libclang=true
                break
            fi
        done
    fi

    if ! $has_libclang; then
        packages+=(llvm)
    else
        success "libclang found"
    fi

    if [ ${#packages[@]} -gt 0 ]; then
        info "Installing: ${packages[*]}"
        brew install "${packages[@]}"
        success "macOS dependencies installed"
    fi
}

install_ollama_linux() {
    if has_cmd ollama; then
        success "Ollama found"
        return
    fi

    info "Installing Ollama via the official install script..."
    curl -fsSL https://ollama.com/install.sh | sh
    success "Ollama installed"
}

# ---------------------------------------------------------------------------
# Linux dependency installation
# ---------------------------------------------------------------------------
install_deps_linux() {
    detect_linux_distro

    case "${DISTRO}" in
        debian)
            info "Detected Debian/Ubuntu-based system"
            local pkgs=(
                libavformat-dev libavcodec-dev libswscale-dev libavutil-dev libavdevice-dev
                pkg-config libclang-dev build-essential yt-dlp
            )
            info "Installing: ${pkgs[*]}"
            sudo apt-get update -qq
            sudo apt-get install -y "${pkgs[@]}"
            success "Dependencies installed via apt"
            ;;
        fedora)
            info "Detected Fedora/RHEL-based system"
            # Fedora needs RPM Fusion for ffmpeg-devel
            if ! rpm -q rpmfusion-free-release >/dev/null 2>&1; then
                warn "Enabling RPM Fusion (needed for FFmpeg dev packages)..."
                sudo dnf install -y \
                    "https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm" \
                    2>/dev/null || true
            fi
            local pkgs=(
                ffmpeg-devel clang-devel pkg-config gcc yt-dlp
            )
            info "Installing: ${pkgs[*]}"
            sudo dnf install -y "${pkgs[@]}"
            success "Dependencies installed via dnf"
            ;;
        arch)
            info "Detected Arch-based system"
            local pkgs=(ffmpeg clang pkg-config base-devel yt-dlp)
            info "Installing: ${pkgs[*]}"
            sudo pacman -Syu --noconfirm --needed "${pkgs[@]}"
            success "Dependencies installed via pacman"
            ;;
        suse)
            info "Detected openSUSE-based system"
            local pkgs=(ffmpeg-devel libclang-devel pkg-config gcc yt-dlp)
            info "Installing: ${pkgs[*]}"
            sudo zypper install -y "${pkgs[@]}"
            success "Dependencies installed via zypper"
            ;;
        *)
            err "Could not detect your Linux distribution."
            err "Please manually install: FFmpeg dev libs, libclang, pkg-config, yt-dlp, Ollama, and a C compiler."
            err "Then re-run this script."
            exit 1
            ;;
    esac
}

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
build_project() {
    info "Building ASCIIVision (this may take a few minutes on first build)..."

    cd "${SCRIPT_DIR}"

    # Set up environment for the build (same logic as the launcher script)
    local uname_s
    uname_s="$(uname -s)"

    if [[ "${uname_s}" == "Darwin" ]]; then
        # Find libclang on macOS
        local candidates=(
            "${LIBCLANG_PATH:-}"
        )
        if has_cmd brew; then
            for formula in llvm llvm@21 llvm@20 llvm@19 llvm@18; do
                local prefix
                prefix="$(brew --prefix "${formula}" 2>/dev/null || true)"
                if [[ -n "${prefix}" && -d "${prefix}/lib" ]]; then
                    candidates+=("${prefix}/lib")
                fi
            done
        fi
        candidates+=(
            "/opt/homebrew/opt/llvm/lib"
            "/usr/local/opt/llvm/lib"
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib"
        )
        for dir in "${candidates[@]}"; do
            if [[ -n "${dir}" && -f "${dir}/libclang.dylib" ]]; then
                export LIBCLANG_PATH="${dir}"
                break
            fi
        done

        # pkg-config and include paths
        if [[ -d /opt/homebrew/lib/pkgconfig ]]; then
            export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:+${PKG_CONFIG_PATH}:}/opt/homebrew/lib/pkgconfig"
        fi
        if [[ -d /usr/local/lib/pkgconfig ]]; then
            export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:+${PKG_CONFIG_PATH}:}/usr/local/lib/pkgconfig"
        fi
        if [[ -d /opt/homebrew/include ]]; then
            export CPATH="${CPATH:+${CPATH}:}/opt/homebrew/include"
        fi
        if [[ -d /usr/local/include ]]; then
            export CPATH="${CPATH:+${CPATH}:}/usr/local/include"
        fi
    fi

    cargo build --release

    success "Build complete"
}

# ---------------------------------------------------------------------------
# Install binary
# ---------------------------------------------------------------------------
install_binary() {
    mkdir -p "${INSTALL_DIR}"

    local bin_src="${SCRIPT_DIR}/target/release/asciivision"

    if [[ ! -f "${bin_src}" ]]; then
        err "Build artifact not found at ${bin_src}"
        exit 1
    fi

    cp "${bin_src}" "${INSTALL_DIR}/asciivision"
    chmod +x "${INSTALL_DIR}/asciivision"
    success "Installed binary to ${INSTALL_DIR}/asciivision"

    # Check if INSTALL_DIR is in PATH
    if [[ ":${PATH}:" != *":${INSTALL_DIR}:"* ]]; then
        warn "${INSTALL_DIR} is not in your PATH."
        local shell_rc=""
        if [[ -n "${ZSH_VERSION:-}" ]] || [[ "${SHELL:-}" == *zsh ]]; then
            shell_rc="$HOME/.zshrc"
        elif [[ -n "${BASH_VERSION:-}" ]] || [[ "${SHELL:-}" == *bash ]]; then
            shell_rc="$HOME/.bashrc"
        fi

        if [[ -n "${shell_rc}" ]]; then
            echo '' >> "${shell_rc}"
            echo '# ASCIIVision' >> "${shell_rc}"
            echo 'export PATH="$HOME/.local/bin:$PATH"' >> "${shell_rc}"
            success "Added ${INSTALL_DIR} to PATH in ${shell_rc}"
            warn "Run 'source ${shell_rc}' or open a new terminal for it to take effect."
        else
            warn "Add this to your shell config:"
            warn "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        fi
    fi
}

# ---------------------------------------------------------------------------
# .env setup
# ---------------------------------------------------------------------------
setup_env() {
    if [[ -f "${SCRIPT_DIR}/.env" ]]; then
        success ".env file already exists"
    elif [[ -f "${SCRIPT_DIR}/.env.example" ]]; then
        cp "${SCRIPT_DIR}/.env.example" "${SCRIPT_DIR}/.env"
        info "Created .env from template (edit it to add your API keys)"
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    echo ""
    printf "${BOLD}========================================${RESET}\n"
    printf "${BOLD}   ASCIIVision Installer${RESET}\n"
    printf "${BOLD}========================================${RESET}\n"
    echo ""

    detect_os

    # 1. Rust
    if ! check_rust; then
        install_rust
    fi

    # 2. System dependencies
    case "${OS}" in
        macos) install_deps_macos ;;
        linux)
            install_deps_linux
            install_ollama_linux
            ;;
    esac

    # 3. Build
    build_project

    # 4. Install binary to PATH
    install_binary

    # 5. Set up .env
    setup_env

    echo ""
    printf "${BOLD}========================================${RESET}\n"
    printf "${GREEN}${BOLD}   Installation complete!${RESET}\n"
    printf "${BOLD}========================================${RESET}\n"
    echo ""
    info "Run the app:  asciivision"
    info "Or from repo: ./asciivision"
    echo ""
    info "Optional: edit .env to add AI API keys (not required for video/effects/sysmon)"
    info "YouTube video loading is available via /youtube now that yt-dlp is installed"
    info "Ollama local-model routing is available via F2 or /ollama once your models are pulled"
    echo ""

    if [[ ":${PATH}:" != *":${INSTALL_DIR}:"* ]]; then
        warn "Remember to open a new terminal or run:"
        warn "  source ~/.zshrc   (or ~/.bashrc)"
        echo ""
    fi
}

main "$@"
