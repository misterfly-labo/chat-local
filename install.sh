#!/usr/bin/env bash
# Installeur de chat-local pour Arch Linux (et dérivés).
#
#   ./install.sh              → installation utilisateur  (~/.local)
#   ./install.sh --system     → installation système      (/usr, via sudo)
#   ./install.sh --uninstall  → désinstalle
#
# L'installation crée :
#   - le binaire chat-local
#   - l'icône chat-local.svg (thème hicolor)
#   - le lanceur chat-local.desktop (l'entrée à épingler dans le dock)
set -euo pipefail

MODE="user"
case "${1:-}" in
    --system)   MODE="system" ;;
    --uninstall) MODE="uninstall" ;;
    "") ;;
    *) echo "Usage : $0 [--system|--uninstall]" >&2; exit 1 ;;
esac

APP=chat-local

if [ "$MODE" = "uninstall" ]; then
    for prefix in "$HOME/.local" /usr/local /usr; do
        rm -fv "$prefix/bin/$APP" \
               "$prefix/share/applications/$APP.desktop" \
               "$prefix/share/icons/hicolor/scalable/apps/$APP.svg"
    done
    command -v update-desktop-database >/dev/null && update-desktop-database -q ~/.local/share/applications /usr/share/applications 2>/dev/null || true
    echo "✅ $APP désinstallé."
    exit 0
fi

cd "$(dirname "$0")"

if [ "$MODE" = "system" ]; then
    PREFIX="/usr/local"
    SUDO="sudo"
    [ "$(id -u)" -eq 0 ] && SUDO=""
else
    PREFIX="$HOME/.local"
    SUDO=""
fi
BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/scalable/apps"

# 1. Compilation
echo "==> Compilation (cargo build --release)…"
cargo build --release

# 2. Installation du binaire
echo "==> Installation du binaire dans $BIN_DIR"
mkdir -p "$BIN_DIR"
if [ -n "$SUDO" ]; then
    $SUDO install -Dm755 target/release/$APP -t "$BIN_DIR"
else
    install -Dm755 target/release/$APP -t "$BIN_DIR"
fi

# 3. Icône + lanceur (.desktop), le « lien » pour le dock
echo "==> Installation de l'icône et du lanceur"
mkdir -p "$APP_DIR" "$ICON_DIR"
if [ -n "$SUDO" ]; then
    $SUDO install -Dm644 "$APP.svg" -t "$ICON_DIR"
    $SUDO install -Dm644 "$APP.desktop" -t "$APP_DIR"
else
    install -Dm644 "$APP.svg" -t "$ICON_DIR"
    install -Dm644 "$APP.desktop" -t "$APP_DIR"
fi

# 4. Rafraîchir les caches des environnements de bureau
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true

echo ""
echo "✅ Installation terminée :"
echo "   - binaire  : $BIN_DIR/$APP"
echo "   - lanceur  : $APP_DIR/$APP.desktop"
echo "   - icône    : $ICON_DIR/$APP.svg"
echo ""
echo "« Chat Lemonade » apparaît dans le lanceur d'applications ;"
echo "épin-le dans le dock depuis le menu contextuel du dock."
