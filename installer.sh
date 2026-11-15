#!/usr/bin/env bash
# ======================================================================
#  Installeur de chat-local — UN SEUL FICHIER, compatible Arch Linux.
#
#  Double-cliquez sur ce fichier dans le gestionnaire de fichiers :
#  il s'ouvre dans une fenêtre de terminal et procède à l'installation
#  complète (binaire, icône, lanceur pour le dock).
#
#  Depuis un terminal :
#    ./installer.sh               → installation utilisateur (~/.local)
#    ./installer.sh --system      → installation système (/usr/local, sudo)
#    ./installer.sh --uninstall   → désinstallation
# ======================================================================
set -euo pipefail

APP=chat-local

# ----------------------------------------------------------------------
# Double-clic ? (pas de terminal attaché) → se relancer dans un terminal
# ----------------------------------------------------------------------
if [ ! -t 0 ] && [ ! -t 1 ] && [ "${INSTALLER_IN_TERMINAL:-}" != "1" ]; then
    for term in foot kitty alacritty ghostty wezterm gnome-terminal konsole xterm; do
        case "$term" in
            foot)           exec foot -e env INSTALLER_IN_TERMINAL=1 bash "$0" "$@" ;;
            kitty)          exec kitty bash -c "INSTALLER_IN_TERMINAL=1 bash '$(readlink -f "$0")' $*; echo; read -n 1 -s -r -p 'Terminé — appuyez sur une touche pour fermer…'" ;;
            alacritty)      exec alacritty -e env INSTALLER_IN_TERMINAL=1 bash "$0" "$@" ;;
            ghostty)        exec ghostty -e env INSTALLER_IN_TERMINAL=1 bash "$0" "$@" ;;
            wezterm)        exec wezterm start -- env INSTALLER_IN_TERMINAL=1 bash "$0" "$@" ;;
            gnome-terminal) exec gnome-terminal -- env INSTALLER_IN_TERMINAL=1 bash "$0" "$@" ;;
            konsole)        exec konsole -e env INSTALLER_IN_TERMINAL=1 bash "$0" "$@" ;;
            xterm)          exec xterm -e env INSTALLER_IN_TERMINAL=1 bash "$0" "$@" ;;
        esac
    done
    echo "Aucun terminal trouvé : lancez ce script depuis un terminal." >&2
    exit 1
fi

# ----------------------------------------------------------------------
# Mode d'installation
# ----------------------------------------------------------------------
MODE="user"
case "${1:-}" in
    --system)   MODE="system" ;;
    --uninstall) MODE="uninstall" ;;
    "") ;;
    *) echo "Usage : $0 [--system|--uninstall]" >&2; exit 1 ;;
esac

# Toujours travailler depuis le dossier du script (double-clic friendly).
cd "$(dirname "$(readlink -f "$0")")"

if [ "$MODE" = "uninstall" ]; then
    echo "==> Désinstallation…"
    for prefix in "$HOME/.local" /usr/local /usr; do
        rm -fv "$prefix/bin/$APP" \
               "$prefix/share/applications/$APP.desktop" \
               "$prefix/share/icons/hicolor/scalable/apps/$APP.svg"
    done
    command -v update-desktop-database >/dev/null && \
        update-desktop-database -q ~/.local/share/applications /usr/share/applications 2>/dev/null || true
    echo ""
    echo "✅ $APP désinstallé."
    pause_or_exit
    exit 0
fi

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

# Utilitaire : marquer une pause si l'utilisateur nous regarde dans un
# terminal ouvert par double-clic, pour que la fenêtre ne disparaisse pas.
pause_or_exit() {
    if [ "${INSTALLER_IN_TERMINAL:-}" = "1" ]; then
        echo ""
        read -r -p "Appuyez sur Entrée pour fermer…" _
    fi
}

# Piège d'erreur : laisser le temps de lire le message avant fermeture.
trap 'echo ""; echo "❌ Installation échouée — consultez le message ci-dessus."; pause_or_exit' ERR

# ----------------------------------------------------------------------
# 1. Compilation
# ----------------------------------------------------------------------
if ! command -v cargo >/dev/null; then
    echo "❌ Rust/cargo manquant. Installez-le :"
    echo "   sudo pacman -S rust"
    pause_or_exit
    exit 1
fi

echo "==> Vérification du serveur Lemonade (optionnel)…"
if curl -sm 2 http://localhost:13305/v1/health >/dev/null 2>&1; then
    echo "    ✔ serveur détecté sur localhost:13305"
else
    echo "    ⚠ serveur non détecté — l'app s'en plaindra au lancement."
fi

echo "==> Compilation (cargo build --release)…"
cargo build --release

# ----------------------------------------------------------------------
# 2. Installation du binaire
# ----------------------------------------------------------------------
echo "==> Installation du binaire dans $BIN_DIR"
mkdir -p "$BIN_DIR"
if [ -n "$SUDO" ]; then
    $SUDO install -Dm755 target/release/$APP -t "$BIN_DIR"
else
    install -Dm755 target/release/$APP -t "$BIN_DIR"
fi

# ----------------------------------------------------------------------
# 3. Icône + lanceur (.desktop), l'entrée à épingler dans le dock
# ----------------------------------------------------------------------
echo "==> Installation de l'icône et du lanceur"
mkdir -p "$APP_DIR" "$ICON_DIR"
if [ -n "$SUDO" ]; then
    $SUDO install -Dm644 "$APP.svg" -t "$ICON_DIR"
    $SUDO install -Dm644 "$APP.desktop" -t "$APP_DIR"
else
    install -Dm644 "$APP.svg" -t "$ICON_DIR"
    install -Dm644 "$APP.desktop" -t "$APP_DIR"
fi

# ----------------------------------------------------------------------
# 4. Rafraîchir les caches des environnements de bureau
# ----------------------------------------------------------------------
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true

echo ""
echo "✅ Installation terminée :"
echo "   - binaire  : $BIN_DIR/$APP"
echo "   - lanceur  : $APP_DIR/$APP.desktop"
echo "   - icône    : $ICON_DIR/$APP.svg"
echo ""
echo "« Chat Lemonade » apparaît dans le lanceur d'applications ;"
echo "épin-le dans le dock depuis son menu contextuel."
pause_or_exit
