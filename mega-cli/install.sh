#!/bin/bash

# MEGA-CLI Installation Script

set -e

echo "🚀 Installing MEGA-CLI..."

# Check if Rust is installed
if ! command -v cargo &> /dev/null; then
    echo "❌ Rust is not installed. Please install Rust from https://rustup.rs/"
    exit 1
fi

# Check if FFmpeg is installed
if ! command -v ffmpeg &> /dev/null; then
    echo "⚠️  FFmpeg not found. Installing..."

    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS
        if command -v brew &> /dev/null; then
            brew install ffmpeg
        else
            echo "❌ Homebrew not found. Please install FFmpeg manually:"
            echo "   https://ffmpeg.org/download.html"
            exit 1
        fi
    elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
        # Linux
        if command -v apt-get &> /dev/null; then
            sudo apt-get update
            sudo apt-get install -y libavformat-dev libavcodec-dev libswscale-dev libavutil-dev pkg-config
        else
            echo "❌ apt-get not found. Please install FFmpeg dev libraries manually."
            exit 1
        fi
    else
        echo "❌ Unsupported OS. Please install FFmpeg manually."
        exit 1
    fi
fi

# Build the project
echo "🔨 Building MEGA-CLI..."
cargo build --release

# Install to user bin
INSTALL_DIR="$HOME/.local/bin"
mkdir -p "$INSTALL_DIR"

echo "📦 Installing to $INSTALL_DIR/mega-cli..."
cp target/release/mega-cli "$INSTALL_DIR/mega-cli"
chmod +x "$INSTALL_DIR/mega-cli"

# Check if .local/bin is in PATH
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "⚠️  $INSTALL_DIR is not in your PATH."
    echo "   Add this line to your ~/.bashrc or ~/.zshrc:"
    echo ""
    echo '   export PATH="$HOME/.local/bin:$PATH"'
    echo ""
fi

# Setup .env if it doesn't exist
if [ ! -f .env ]; then
    echo "📝 Creating .env file from template..."
    cp .env.example .env
    echo "⚠️  Please edit .env and add your API keys!"
fi

echo ""
echo "✅ Installation complete!"
echo ""
echo "Next steps:"
echo "  1. Edit .env and add your API keys"
echo "  2. Run: mega-cli"
echo ""
echo "For help: mega-cli --help"
